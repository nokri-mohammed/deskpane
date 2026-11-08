//! Edits a Claude Code settings document (`~/.claude/settings.json`) in
//! memory: adds or removes one command hook handler on a fixed list of hook
//! events, never touching any other key, event or handler, and keeping their
//! order. Pure: values in, values out, no disk access. The thin file wrapper
//! is settings_file.rs.

use serde_json::{json, Value};

// the hook events the handler is installed on, in the order they are written
pub const AGENT_HOOK_EVENTS: [&str; 10] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

// the only event with a matcher: just the notification types that matter
pub const NOTIFICATION_MATCHER: &str =
    "permission_prompt|idle_prompt|elicitation_dialog|elicitation_url_dialog|agent_needs_input";

// the program names that make a handler ours (Linux and macOS, Windows)
const OUR_PROGRAMS: [&str; 2] = ["deskpane", "deskpane.exe"];
// the first argument that makes a handler ours
const OUR_FIRST_ARG: &str = "agent";

/*
 * one command hook handler in exec form (no shell)
 *
 * command: absolute path of the program, e.g. /usr/local/bin/deskpane
 * args: its arguments
 * timeout_secs: Claude Code kills the hook after this long
 * returns: {"type":"command","command":..,"args":[..],"timeout":..,"onFailure":"continue"}
 */
pub fn command_handler(command: &str, args: &[String], timeout_secs: u64) -> Value {
    json!({
        "type": "command",
        "command": command,
        "args": args,
        "timeout": timeout_secs,
        "onFailure": "continue"
    })
}

/*
 * true when a handler is ours: its command's file name is deskpane or
 * deskpane.exe (any case) and its first argument is agent
 */
pub fn is_agent_handler(handler: &Value) -> bool {
    let command_found = handler.get("command");
    let command = match command_found {
        Some(c) => c,
        None => return false,
    };
    let command_text_found = command.as_str();
    let command_text = match command_text_found {
        Some(t) => t,
        None => return false,
    };

    // the file name: the part after the last / or \
    let split = command_text.rsplit_once(['/', '\\']);
    let program = match split {
        Some((_, name)) => name,
        None => command_text,
    };
    let program_lower = program.to_ascii_lowercase();
    let mut is_our_program = false;
    for name in OUR_PROGRAMS {
        if program_lower == name {
            is_our_program = true;
        }
    }
    if !is_our_program {
        return false;
    }

    let args_found = handler.get("args");

    let args = match args_found {
        Some(a) => a,
        None => return false,
    };
    let list_found = args.as_array();
    let list = match list_found {
        Some(l) => l,
        None => return false,
    };
    if list.is_empty() {
        return false;
    }
    list[0] == OUR_FIRST_ARG
}

// a new matcher group holding only our handler; Notification gets the matcher
fn new_group(event: &str, handler: &Value) -> Value {
    if event == "Notification" {
        return json!({"matcher": NOTIFICATION_MATCHER, "hooks": [handler.clone()]});
    }
    json!({"hooks": [handler.clone()]})
}

/*
 * replaces every handler of ours in an event's groups with the new one
 *
 * groups: the event's list of matcher groups
 * handler: the new handler
 * returns: true if at least one was replaced. Groups that are not objects,
 *          or whose hooks is not a list, are left alone.
 */
fn replace_ours(groups: &mut [Value], handler: &Value) -> bool {
    let mut replaced = false;
    for group in groups.iter_mut() {
        let hooks_found = group.get_mut("hooks");
        let hooks = match hooks_found {
            Some(h) => h,
            None => continue,
        };
        let list_found = hooks.as_array_mut();
        let list = match list_found {
            Some(l) => l,
            None => continue,
        };
        for existing in list.iter_mut() {
            if is_agent_handler(existing) {
                *existing = handler.clone();
                replaced = true;
            }
        }
    }
    replaced
}

/*
 * adds our handler to every event in AGENT_HOOK_EVENTS, never overwriting
 *
 * settings: the whole settings document
 * handler: our handler (see command_handler)
 * returns: the merged document, or Err when settings is not an object,
 *          hooks is not an object, or one of our events is not a list. A
 *          handler of ours already there is replaced in place, so merging
 *          twice changes nothing. Everything else keeps its value and order.
 */
pub fn merge_agent_hooks(settings: Value, handler: Value) -> Result<Value, String> {
    let mut settings = settings;
    let root_found = settings.as_object_mut();
    let root = match root_found {
        Some(r) => r,
        None => return Err("the settings file is not a JSON object".to_string()),
    };

    // a missing hooks key is added at the end
    if !root.contains_key("hooks") {
        root.insert("hooks".to_string(), json!({}));
    }
    let hooks_value_found = root.get_mut("hooks");
    let hooks_value = match hooks_value_found {
        Some(h) => h,
        None => return Err("hooks could not be added".to_string()),
    };
    let hooks_found = hooks_value.as_object_mut();
    let hooks = match hooks_found {
        Some(h) => h,
        None => return Err("\"hooks\" is not an object".to_string()),
    };

    // check every event first, so a refusal changes nothing
    for event in AGENT_HOOK_EVENTS {
        // a missing event is fine: it is added below
        let event_value = hooks.get(event);
        let is_list = match event_value {
            Some(value) => value.is_array(),
            None => true,
        };
        if !is_list {
            return Err(format!("\"hooks.{event}\" is not a list"));
        }
    }

    for event in AGENT_HOOK_EVENTS {
        let existing = hooks.get_mut(event);
        match existing {
            Some(value) => {
                let groups_found = value.as_array_mut();
                let groups = match groups_found {
                    Some(g) => g,
                    None => return Err(format!("\"hooks.{event}\" is not a list")),
                };
                let replaced = replace_ours(groups, &handler);
                if !replaced {
                    groups.push(new_group(event, &handler));
                }
            }
            None => {
                hooks.insert(event.to_string(), json!([new_group(event, &handler)]));
            }
        }
    }

    Ok(settings)
}

/*
 * removes our handlers from one matcher group
 *
 * group: one matcher group
 * returns: how many were removed (0 for a group that is not an object or
 *          whose hooks is not a list)
 */
fn remove_ours_from_group(group: &mut Value) -> usize {
    let hooks_found = group.get_mut("hooks");
    let hooks = match hooks_found {
        Some(h) => h,
        None => return 0,
    };
    let list_found = hooks.as_array_mut();
    let list = match list_found {
        Some(l) => l,
        None => return 0,
    };

    let old = std::mem::take(list);
    let mut removed = 0;
    for handler in old {
        if is_agent_handler(&handler) {
            removed += 1;
        } else {
            list.push(handler);
        }
    }
    removed
}

// true when a group's hooks list is empty
fn group_is_empty(group: &Value) -> bool {
    let hooks_found = group.get("hooks");
    let hooks = match hooks_found {
        Some(h) => h,
        None => return false,
    };
    let as_list = hooks.as_array();
    match as_list {
        Some(list) => list.is_empty(),
        None => false,
    }
}

/*
 * removes every handler of ours, from any event
 *
 * settings: the whole settings document
 * returns: the document without our handlers; groups and event keys left
 *          empty by that are removed too. Err when settings or hooks is not
 *          an object. Events that are not lists are left alone.
 */
pub fn remove_agent_hooks(settings: Value) -> Result<Value, String> {
    let mut settings = settings;
    let root_found = settings.as_object_mut();
    let root = match root_found {
        Some(r) => r,
        None => return Err("the settings file is not a JSON object".to_string()),
    };
    let hooks_value_found = root.get_mut("hooks");
    let hooks_value = match hooks_value_found {
        Some(h) => h,
        None => return Ok(settings),
    };
    let hooks_found = hooks_value.as_object_mut();
    let hooks = match hooks_found {
        Some(h) => h,
        None => return Err("\"hooks\" is not an object".to_string()),
    };

    let mut emptied: Vec<String> = Vec::new();
    for (event, value) in hooks.iter_mut() {
        let groups_found = value.as_array_mut();
        let groups = match groups_found {
            Some(g) => g,
            None => continue,
        };

        // keep each group, unless removing ours left it empty
        let old = std::mem::take(groups);
        let mut removed_any = false;
        for mut group in old {
            let removed = remove_ours_from_group(&mut group);
            if removed > 0 {
                removed_any = true;
                if group_is_empty(&group) {
                    continue;
                }
            }
            groups.push(group);
        }

        if removed_any && groups.is_empty() {
            emptied.push(event.clone());
        }
    }

    // shift_remove keeps the order of the other events
    for event in &emptied {
        hooks.shift_remove(event);
    }

    Ok(settings)
}

/*
 * the time part of a backup file name, in UTC: YYYYMMDD-HHMMSS
 *
 * unix_secs: seconds since the Unix epoch
 */
pub fn backup_suffix(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let second_of_day = unix_secs % 86_400;
    let hour = second_of_day / 3600;
    let minute = (second_of_day % 3600) / 60;
    let second = second_of_day % 60;

    /*
     * days since 1970-01-01 to a calendar date (Howard Hinnant's
     * civil_from_days): count in 400-year eras that start on 0000-03-01, so
     * the leap day falls at the end of each year
     */
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;

    let month;
    if month_from_march < 10 {
        month = month_from_march + 3;
    } else {
        month = month_from_march - 9;
    }

    let mut year = year_of_era + era * 400;
    if month <= 2 {
        year += 1;
    }

    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // our handler, as hooks-snippet builds it
    fn ours(daemon: &str) -> Value {
        let args = vec![
            "agent".to_string(),
            "--daemon".to_string(),
            daemon.to_string(),
            "--device-id".to_string(),
            "dp-3f9a0c41".to_string(),
        ];
        command_handler("/usr/local/bin/deskpane", &args, 2)
    }

    // someone else's hook handler
    fn theirs(command: &str) -> Value {
        json!({"type": "command", "command": command})
    }

    // parses JSON text in a test
    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    // the keys of an object, in order
    fn keys(value: &Value) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for (key, _) in value.as_object().unwrap() {
            names.push(key.clone());
        }
        names
    }

    // ---- handler ----

    #[test]
    fn command_handler_has_exec_form_timeout_and_continue() {
        let handler = ours("192.168.1.50:8765");

        assert_eq!(
            serde_json::to_string(&handler).unwrap(),
            r#"{"type":"command","command":"/usr/local/bin/deskpane","args":["agent","--daemon","192.168.1.50:8765","--device-id","dp-3f9a0c41"],"timeout":2,"onFailure":"continue"}"#
        );
    }

    #[test]
    fn our_handler_is_recognised_on_linux_and_windows() {
        assert!(is_agent_handler(&ours("192.168.1.50:8765")));
        let windows = json!({
            "type": "command",
            "command": "C:\\Program Files\\deskpane\\deskpane.exe",
            "args": ["agent", "--daemon", "192.168.1.50:8765"]
        });
        assert!(is_agent_handler(&windows));
        let upper = json!({"command": "C:\\Tools\\DESKPANE.EXE", "args": ["agent"]});
        assert!(is_agent_handler(&upper));
        let bare = json!({"command": "deskpane", "args": ["agent"]});
        assert!(is_agent_handler(&bare));
    }

    #[test]
    fn other_handlers_are_not_ours() {
        // a name that only ends in deskpane
        assert!(!is_agent_handler(
            &json!({"command": "/usr/bin/notdeskpane", "args": ["agent"]})
        ));
        // our binary, but not the agent command
        assert!(!is_agent_handler(
            &json!({"command": "/usr/local/bin/deskpane", "args": ["report"]})
        ));
        assert!(!is_agent_handler(
            &json!({"command": "/usr/local/bin/deskpane"})
        ));
        assert!(!is_agent_handler(
            &json!({"command": "/usr/local/bin/deskpane", "args": []})
        ));
        // shell form, other tools, junk
        assert!(!is_agent_handler(&theirs("deskpane agent --daemon x")));
        assert!(!is_agent_handler(&theirs("/home/user/bin/lint.sh")));
        assert!(!is_agent_handler(&json!("deskpane")));
        assert!(!is_agent_handler(&json!({"command": 5, "args": ["agent"]})));
    }

    // ---- merge ----

    #[test]
    fn merge_into_empty_settings_adds_all_events() {
        let merged = merge_agent_hooks(json!({}), ours("192.168.1.50:8765")).unwrap();

        let hooks = &merged["hooks"];
        let mut expected: Vec<String> = Vec::new();
        for event in AGENT_HOOK_EVENTS {
            expected.push(event.to_string());
        }
        assert_eq!(keys(hooks), expected);
        for event in AGENT_HOOK_EVENTS {
            let groups = hooks[event].as_array().unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0]["hooks"], json!([ours("192.168.1.50:8765")]));
            if event == "Notification" {
                assert_eq!(groups[0]["matcher"], NOTIFICATION_MATCHER);
                assert_eq!(keys(&groups[0]), vec!["matcher", "hooks"]);
            } else {
                assert!(groups[0].get("matcher").is_none());
            }
        }
    }

    #[test]
    fn merge_keeps_other_hooks_and_keys_in_order() {
        let settings = parse(
            r#"{
              "model": "opus",
              "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "/home/user/bin/notify.sh"}]}],
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "/home/user/bin/guard.sh"}]}],
                "PreCompact": [{"hooks": [{"type": "command", "command": "/home/user/bin/save.sh"}]}]
              },
              "statusLine": {"type": "command", "command": "/home/user/bin/status.sh"},
              "permissions": {"allow": ["Bash(ls:*)"]}
            }"#,
        );

        let merged = merge_agent_hooks(settings.clone(), ours("192.168.1.50:8765")).unwrap();

        // top-level keys keep their order and values
        assert_eq!(
            keys(&merged),
            vec!["model", "hooks", "statusLine", "permissions"]
        );
        assert_eq!(merged["statusLine"], settings["statusLine"]);
        assert_eq!(merged["permissions"], settings["permissions"]);

        // existing events first, in their order, then the new ones
        let hook_keys = keys(&merged["hooks"]);
        assert_eq!(&hook_keys[..3], &["Stop", "PreToolUse", "PreCompact"]);
        assert_eq!(hook_keys.len(), 3 + AGENT_HOOK_EVENTS.len() - 2);

        // their groups stay first and unchanged; ours is appended as its own group
        let stop = merged["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0], settings["hooks"]["Stop"][0]);
        assert_eq!(stop[1]["hooks"][0], ours("192.168.1.50:8765"));
        let pre = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre[0], settings["hooks"]["PreToolUse"][0]);
        assert_eq!(
            merged["hooks"]["PreCompact"],
            settings["hooks"]["PreCompact"]
        );
    }

    #[test]
    fn merge_twice_is_unchanged() {
        let once = merge_agent_hooks(json!({"hooks": {}}), ours("192.168.1.50:8765")).unwrap();

        let twice = merge_agent_hooks(once.clone(), ours("192.168.1.50:8765")).unwrap();

        assert_eq!(twice, once);
        assert_eq!(
            serde_json::to_string(&twice).unwrap(),
            serde_json::to_string(&once).unwrap()
        );
    }

    #[test]
    fn merge_replaces_our_old_handler_in_place() {
        // our old handler sits between two of theirs, in a group with a matcher
        let settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "*", "hooks": [theirs("/a.sh"), ours("192.168.1.40:8765"), theirs("/b.sh")]}
                ]
            }
        });

        let merged = merge_agent_hooks(settings, ours("192.168.1.50:8765")).unwrap();

        let groups = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "no second group is added");
        assert_eq!(groups[0]["matcher"], "*");
        assert_eq!(
            groups[0]["hooks"],
            json!([theirs("/a.sh"), ours("192.168.1.50:8765"), theirs("/b.sh")])
        );
    }

    #[test]
    fn merge_refuses_non_object_settings() {
        assert!(merge_agent_hooks(json!([1, 2]), ours("x:1")).is_err());
        assert!(merge_agent_hooks(json!("text"), ours("x:1")).is_err());
    }

    #[test]
    fn merge_refuses_non_object_hooks() {
        let refused = merge_agent_hooks(json!({"hooks": []}), ours("x:1"));
        match refused {
            Ok(_) => panic!("expected a refusal"),
            Err(reason) => assert!(reason.contains("hooks")),
        }
    }

    #[test]
    fn merge_refuses_non_array_event() {
        let settings = json!({"hooks": {"Stop": {"hooks": []}}});
        let refused = merge_agent_hooks(settings, ours("x:1"));
        match refused {
            Ok(_) => panic!("expected a refusal"),
            Err(reason) => assert!(reason.contains("Stop")),
        }
    }

    #[test]
    fn merge_leaves_odd_groups_alone() {
        // a group that is not an object, or whose hooks is not a list, is kept as is
        let settings = json!({"hooks": {"Stop": ["odd", {"hooks": "odd"}]}});

        let merged = merge_agent_hooks(settings, ours("x:1")).unwrap();

        let stop = merged["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop[0], json!("odd"));
        assert_eq!(stop[1], json!({"hooks": "odd"}));
        assert_eq!(stop[2]["hooks"][0], ours("x:1"));
    }

    // ---- remove ----

    #[test]
    fn remove_deletes_only_our_handlers() {
        let settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Bash", "hooks": [theirs("/guard.sh"), ours("x:1")]}
                ],
                "PreCompact": [{"hooks": [theirs("/save.sh")]}]
            },
            "model": "opus"
        });

        let removed = remove_agent_hooks(settings).unwrap();

        assert_eq!(
            removed,
            json!({
                "hooks": {
                    "PreToolUse": [{"matcher": "Bash", "hooks": [theirs("/guard.sh")]}],
                    "PreCompact": [{"hooks": [theirs("/save.sh")]}]
                },
                "model": "opus"
            })
        );
        assert_eq!(keys(&removed), vec!["hooks", "model"]);
    }

    #[test]
    fn remove_drops_groups_left_empty() {
        let settings = json!({
            "hooks": {
                "Stop": [
                    {"hooks": [theirs("/notify.sh")]},
                    {"hooks": [ours("x:1")]}
                ],
                "SessionEnd": [{"hooks": [ours("x:1")]}],
                "PreCompact": [{"hooks": []}]
            }
        });

        let removed = remove_agent_hooks(settings).unwrap();

        // Stop keeps its other group; SessionEnd held only ours, so the key goes;
        // an empty group that was never ours stays
        assert_eq!(
            removed,
            json!({
                "hooks": {
                    "Stop": [{"hooks": [theirs("/notify.sh")]}],
                    "PreCompact": [{"hooks": []}]
                }
            })
        );
        assert_eq!(keys(&removed["hooks"]), vec!["Stop", "PreCompact"]);
    }

    #[test]
    fn merge_then_remove_gives_back_the_other_hooks() {
        let settings = parse(
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/n.sh"}]}]}, "theme": "dark"}"#,
        );

        let merged = merge_agent_hooks(settings.clone(), ours("x:1")).unwrap();
        let removed = remove_agent_hooks(merged).unwrap();

        assert_eq!(removed, settings);
        assert_eq!(
            serde_json::to_string(&removed).unwrap(),
            serde_json::to_string(&settings).unwrap()
        );
    }

    #[test]
    fn remove_without_hooks_changes_nothing() {
        let settings = json!({"model": "opus"});
        assert_eq!(remove_agent_hooks(settings.clone()).unwrap(), settings);
    }

    #[test]
    fn remove_refuses_non_object_settings_and_hooks() {
        assert!(remove_agent_hooks(json!([])).is_err());
        assert!(remove_agent_hooks(json!({"hooks": "x"})).is_err());
    }

    #[test]
    fn remove_leaves_non_array_events_alone() {
        let settings = json!({"hooks": {"Stop": {"odd": true}}});
        assert_eq!(remove_agent_hooks(settings.clone()).unwrap(), settings);
    }

    // ---- backup name ----

    #[test]
    fn backup_suffix_is_utc_date_and_time() {
        assert_eq!(backup_suffix(0), "19700101-000000");
        assert_eq!(backup_suffix(1794152400), "20261108-154000");
        // a leap day, and the last second of a non-leap century year's February
        assert_eq!(backup_suffix(951782400), "20000229-000000");
        assert_eq!(backup_suffix(4107542399), "21000228-235959");
    }
}
