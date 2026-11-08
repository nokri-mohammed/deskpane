//! `deskpane agent`: the pure parts of the Claude Code hook path. Reads the
//! hook JSON, maps the event to a state and a short "doing" text, and builds
//! the agent packet. Nothing here touches the network, stdin or the
//! environment, so tests feed it plain data. The orchestration (stdin, env,
//! deadline, send) stays in main.rs.

use daemon::contract::{
    is_format_char, AgentPacket, AgentState, AGENT_PACKET_VERSION, DOING_THINKING, MAX_DOING_BYTES,
    MAX_PROJECT_BYTES, PACKET_KIND_AGENT, SID_LEN, SUB_LEN,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::cut_to_bytes;

/// The few hook input fields we read; serde skips everything else (prompt,
/// tool input and output, transcript path, ...), so it never reaches us.
#[derive(Debug, Clone, Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    // StopFailure: a word from a fixed list; other events may carry other shapes
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    // Stop: only its length is used
    #[serde(default)]
    pub background_tasks: Option<serde_json::Value>,
    #[serde(default)]
    pub cwd: Option<String>,
    // set only inside a subagent; hashed into sub, never sent as is
    #[serde(default)]
    pub agent_id: Option<serde_json::Value>,
}

/*
 * the subagent key sent on the wire: the first 8 hex characters of SHA-256
 * over a fixed prefix and the hook's agent_id, so parallel subagents of one
 * session can each be waiting on the user
 *
 * agent_id: the hook input's agent_id (only set inside a subagent)
 * returns: Some(8 hex) for a non-empty text id, else None (the main agent)
 */
pub fn sub_key(agent_id: &Option<serde_json::Value>) -> Option<String> {
    let value = match agent_id {
        Some(v) => v,
        None => return None,
    };
    let as_text = value.as_str();
    let id = match as_text {
        Some(t) => t,
        None => return None,
    };
    if id.is_empty() {
        return None;
    }

    let input = format!("{SUB_HASH_PREFIX}{id}");
    let digest = Sha256::digest(input.as_bytes());
    let hex = format!("{digest:x}");
    Some(hex[..SUB_LEN].to_string())
}

/*
 * parses the hook JSON from stdin into the few fields we read
 *
 * bytes: stdin as read (already capped)
 * returns: the input, or None for junk or a missing or empty session_id
 */
pub fn parse_hook_input(bytes: &[u8]) -> Option<HookInput> {
    let parsed = serde_json::from_slice::<HookInput>(bytes);
    let input = match parsed {
        Ok(i) => i,
        Err(_) => return None,
    };

    // every row needs a session to belong to
    if input.session_id.is_empty() {
        return None;
    }
    Some(input)
}

/*
 * maps one hook event to the state and "doing" text to send (the table in
 * section 1 of the plan)
 *
 * input: the parsed hook input
 * returns: (state, doing), or None to send nothing for this event
 */
pub fn map_event(input: &HookInput) -> Option<(AgentState, String)> {
    match input.hook_event_name.as_str() {
        "SessionStart" => {
            let source = text_or_empty(&input.source);
            if source == "compact" {
                return Some((AgentState::Working, "compacting".to_string()));
            }
            Some((AgentState::Idle, String::new()))
        }
        "UserPromptSubmit" => Some((AgentState::Working, DOING_THINKING.to_string())),
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" => {
            Some((AgentState::Working, tool_label(&input.tool_name)))
        }
        "PermissionRequest" => {
            // the label is cut so "allow " and "?" always fit
            let label = tool_label(&input.tool_name);
            let room = MAX_DOING_BYTES - ALLOW_PREFIX.len() - ALLOW_SUFFIX.len();
            let short = cut_to_bytes(&label, room);
            Some((
                AgentState::NeedsYou,
                format!("{ALLOW_PREFIX}{short}{ALLOW_SUFFIX}"),
            ))
        }
        "Notification" => {
            let kind = text_or_empty(&input.notification_type);
            match kind.as_str() {
                "permission_prompt" => Some((AgentState::NeedsYou, "permission".to_string())),
                "elicitation_dialog" | "elicitation_url_dialog" => {
                    Some((AgentState::NeedsYou, "question".to_string()))
                }
                "agent_needs_input" => Some((AgentState::NeedsYou, "input".to_string())),
                "idle_prompt" => Some((AgentState::Done, String::new())),
                _ => None,
            }
        }
        "Stop" => {
            if has_background_tasks(&input.background_tasks) {
                return Some((AgentState::Working, "background".to_string()));
            }
            Some((AgentState::Done, String::new()))
        }
        "StopFailure" => {
            let word = stop_failure_word(&input.error);
            Some((AgentState::Error, format!("API error: {word}")))
        }
        "SessionEnd" => Some((AgentState::Ended, String::new())),
        _ => None,
    }
}

// "allow Bash?" around the tool label on a permission request
const ALLOW_PREFIX: &str = "allow ";
const ALLOW_SUFFIX: &str = "?";
// the label for an MCP tool: the server name is user-chosen and can say too much
const MCP_LABEL: &str = "MCP";
const MCP_TOOL_PREFIX: &str = "mcp__";
// the label when the hook names no tool
const UNKNOWN_TOOL_LABEL: &str = "tool";
// the StopFailure error words we pass on; anything else is sent as "unknown"
const STOP_FAILURE_ERRORS: [&str; 6] = [
    "rate_limit",
    "overloaded",
    "authentication_failed",
    "billing_error",
    "server_error",
    "unknown",
];
// prefix hashed with the session id, so the raw id never leaves the machine
const SID_HASH_PREFIX: &str = "deskpane-agent-session-v1:";
// prefix hashed with a subagent's agent_id
const SUB_HASH_PREFIX: &str = "deskpane-agent-sub-v1:";

// the text of an optional field, or "" when it is missing
fn text_or_empty(value: &Option<String>) -> String {
    match value {
        Some(text) => text.clone(),
        None => String::new(),
    }
}

// true when background_tasks is a list with at least one entry
fn has_background_tasks(value: &Option<serde_json::Value>) -> bool {
    let tasks = match value {
        Some(v) => v,
        None => return false,
    };
    let as_list = tasks.as_array();
    let list = match as_list {
        Some(l) => l,
        None => return false,
    };
    !list.is_empty()
}

// the StopFailure error word if it is on the fixed list, else "unknown"
fn stop_failure_word(value: &Option<serde_json::Value>) -> &'static str {
    let error = match value {
        Some(v) => v,
        None => return "unknown",
    };
    let as_text = error.as_str();
    let text = match as_text {
        Some(t) => t,
        None => return "unknown",
    };

    for word in STOP_FAILURE_ERRORS {
        if text == word {
            return word;
        }
    }
    "unknown"
}

/*
 * text with control and format characters (bidi overrides, zero-width)
 * removed, cut to max_bytes on a character boundary. The daemon refuses a
 * packet with either, so they are dropped here instead
 */
fn clean_text(text: &str, max_bytes: usize) -> String {
    let mut printable = String::new();
    for c in text.chars() {
        if !c.is_control() && !is_format_char(c) {
            printable.push(c);
        }
    }
    cut_to_bytes(&printable, max_bytes)
}

/*
 * what the row says the agent is doing for a tool: the built-in tool name as
 * Claude Code sends it, "MCP" for any MCP tool, "tool" when there is no name
 */
pub fn tool_label(tool_name: &Option<String>) -> String {
    let name = text_or_empty(tool_name);
    if name.is_empty() {
        return UNKNOWN_TOOL_LABEL.to_string();
    }
    if name.starts_with(MCP_TOOL_PREFIX) {
        return MCP_LABEL.to_string();
    }

    let label = clean_text(&name, MAX_DOING_BYTES);
    if label.is_empty() {
        return UNKNOWN_TOOL_LABEL.to_string();
    }
    label
}

/*
 * the session key sent on the wire: the first 12 hex characters of SHA-256
 * over a fixed prefix and Claude Code's session_id
 */
pub fn session_sid(session_id: &str) -> String {
    let input = format!("{SID_HASH_PREFIX}{session_id}");
    let digest = Sha256::digest(input.as_bytes());
    let hex = format!("{digest:x}");
    hex[..SID_LEN].to_string()
}

// the last part of a path, for / and \ separators alike; "" for a bare root
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let split = trimmed.rsplit_once(['/', '\\']);
    match split {
        Some((_, last)) => last,
        None => trimmed,
    }
}

/*
 * the project name the row shows
 *
 * alias: --project, wins when given and not empty
 * hide: --hide-project, sends "" whatever else is set
 * project_dir: CLAUDE_PROJECT_DIR, the folder the session started in
 * cwd: the hook's cwd, used when CLAUDE_PROJECT_DIR is missing or empty
 * home: the user's home folder; a session started there sends "" (the
 *       folder name is the user name)
 * returns: at most 32 bytes, no control characters; only a basename, never
 *          a full path (it holds the user name)
 */
pub fn resolve_project(
    alias: &Option<String>,
    hide: bool,
    project_dir: &Option<String>,
    cwd: &Option<String>,
    home: &Option<String>,
) -> String {
    if hide {
        return String::new();
    }

    let alias_text = text_or_empty(alias);
    if !alias_text.is_empty() {
        return clean_text(&alias_text, MAX_PROJECT_BYTES);
    }

    let dir = text_or_empty(project_dir);
    let path;
    if !dir.is_empty() {
        path = dir;
    } else {
        path = text_or_empty(cwd);
    }

    // the home folder's name is the user name: never send it
    let home_text = text_or_empty(home);
    if !home_text.is_empty() && same_folder(&path, &home_text) {
        return String::new();
    }

    clean_text(basename(&path), MAX_PROJECT_BYTES)
}

/*
 * true when two paths name the same folder, ignoring trailing separators,
 * forward versus back slashes and letter case (Windows paths ignore case)
 */
fn same_folder(a: &str, b: &str) -> bool {
    let a_normal = a.replace('\\', "/");
    let b_normal = b.replace('\\', "/");
    let a_trimmed = a_normal.trim_end_matches('/');
    let b_trimmed = b_normal.trim_end_matches('/');
    a_trimmed.eq_ignore_ascii_case(b_trimmed)
}

/*
 * one agent packet, ready to serialize
 *
 * sid: from session_sid
 * device_id: this machine's device id
 * project: from resolve_project
 * state, doing: from map_event
 * t: sender time, milliseconds since the Unix epoch
 * sub: the subagent key from sub_key, None for the main agent
 */
pub fn build_agent_packet(
    sid: &str,
    device_id: &str,
    project: &str,
    state: AgentState,
    doing: &str,
    t: u64,
    sub: Option<String>,
) -> AgentPacket {
    AgentPacket {
        kind: PACKET_KIND_AGENT.to_string(),
        v: AGENT_PACKET_VERSION,
        sid: sid.to_string(),
        device_id: device_id.to_string(),
        project: project.to_string(),
        state: state,
        doing: doing.to_string(),
        t: t,
        ctx: None,
        sub: sub,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon::contract::validate_agent;

    // a hook input as Claude Code sends it, with the given event and extra fields
    fn hook(event: &str, extra: &str) -> HookInput {
        let mut json = format!(
            r#"{{"session_id":"0b9f8c2e-1d4a-4f6b-9a3e-5c7d2e1f0a9b","transcript_path":"/home/user/.claude/projects/x.jsonl","cwd":"/home/user/projects/deskpane","hook_event_name":"{event}""#
        );
        if !extra.is_empty() {
            json.push(',');
            json.push_str(extra);
        }
        json.push('}');
        parse_hook_input(json.as_bytes()).unwrap()
    }

    // the state and doing text map_event gives, or None
    fn mapped(event: &str, extra: &str) -> Option<(AgentState, String)> {
        map_event(&hook(event, extra))
    }

    // (state, doing) as owned values, for comparing
    fn sent(state: AgentState, doing: &str) -> Option<(AgentState, String)> {
        Some((state, doing.to_string()))
    }

    // ---- parsing ----

    #[test]
    fn hook_input_keeps_only_the_fields_we_read() {
        let input = hook(
            "PreToolUse",
            r#""tool_name":"Bash","tool_input":{"command":"rm -rf /tmp/x"},"permission_mode":"default""#,
        );
        assert_eq!(input.hook_event_name, "PreToolUse");
        assert_eq!(input.tool_name, Some("Bash".to_string()));
        assert_eq!(input.cwd, Some("/home/user/projects/deskpane".to_string()));
    }

    #[test]
    fn junk_and_missing_session_id_do_not_parse() {
        assert!(parse_hook_input(b"not json").is_none());
        assert!(parse_hook_input(b"").is_none());
        assert!(parse_hook_input(br#"{"hook_event_name":"Stop"}"#).is_none());
    }

    #[test]
    fn empty_session_id_does_not_parse() {
        let parsed = parse_hook_input(br#"{"session_id":"","hook_event_name":"Stop"}"#);
        assert!(parsed.is_none());
    }

    #[test]
    fn post_tool_use_failure_with_an_error_object_still_parses() {
        let input = hook(
            "PostToolUseFailure",
            r#""tool_name":"Bash","error":{"message":"exit 1"}"#,
        );
        assert_eq!(map_event(&input), sent(AgentState::Working, "Bash"));
    }

    // ---- map_event: one test per row of the mapping table ----

    #[test]
    fn session_start_compact_is_working_compacting() {
        assert_eq!(
            mapped("SessionStart", r#""source":"compact""#),
            sent(AgentState::Working, "compacting")
        );
    }

    #[test]
    fn session_start_other_source_is_idle() {
        for source in ["startup", "resume", "clear", "fork"] {
            let extra = format!(r#""source":"{source}""#);
            assert_eq!(mapped("SessionStart", &extra), sent(AgentState::Idle, ""));
        }
        assert_eq!(mapped("SessionStart", ""), sent(AgentState::Idle, ""));
    }

    #[test]
    fn user_prompt_submit_is_working_thinking() {
        assert_eq!(
            mapped("UserPromptSubmit", r#""prompt":"my secret plan""#),
            sent(AgentState::Working, "thinking")
        );
    }

    #[test]
    fn pre_tool_use_is_working_with_the_tool_label() {
        assert_eq!(
            mapped("PreToolUse", r#""tool_name":"Edit""#),
            sent(AgentState::Working, "Edit")
        );
    }

    #[test]
    fn permission_request_is_needs_you_allow_tool() {
        assert_eq!(
            mapped("PermissionRequest", r#""tool_name":"Bash""#),
            sent(AgentState::NeedsYou, "allow Bash?")
        );
    }

    #[test]
    fn post_tool_use_is_working_with_the_tool_label() {
        assert_eq!(
            mapped(
                "PostToolUse",
                r#""tool_name":"Write","tool_response":{"ok":true}"#
            ),
            sent(AgentState::Working, "Write")
        );
    }

    #[test]
    fn post_tool_use_failure_is_working_with_the_tool_label() {
        assert_eq!(
            mapped("PostToolUseFailure", r#""tool_name":"Read""#),
            sent(AgentState::Working, "Read")
        );
    }

    #[test]
    fn notification_permission_prompt_is_needs_you_permission() {
        assert_eq!(
            mapped(
                "Notification",
                r#""notification_type":"permission_prompt","message":"Claude needs your permission""#
            ),
            sent(AgentState::NeedsYou, "permission")
        );
    }

    #[test]
    fn notification_elicitation_is_needs_you_question() {
        for kind in ["elicitation_dialog", "elicitation_url_dialog"] {
            let extra = format!(r#""notification_type":"{kind}""#);
            assert_eq!(
                mapped("Notification", &extra),
                sent(AgentState::NeedsYou, "question")
            );
        }
    }

    #[test]
    fn notification_agent_needs_input_is_needs_you_input() {
        assert_eq!(
            mapped("Notification", r#""notification_type":"agent_needs_input""#),
            sent(AgentState::NeedsYou, "input")
        );
    }

    #[test]
    fn notification_idle_prompt_is_done() {
        assert_eq!(
            mapped("Notification", r#""notification_type":"idle_prompt""#),
            sent(AgentState::Done, "")
        );
    }

    #[test]
    fn notification_of_another_type_sends_nothing() {
        assert_eq!(
            mapped("Notification", r#""notification_type":"auth_success""#),
            None
        );
        assert_eq!(mapped("Notification", ""), None);
    }

    #[test]
    fn stop_without_background_tasks_is_done() {
        assert_eq!(mapped("Stop", ""), sent(AgentState::Done, ""));
        assert_eq!(
            mapped("Stop", r#""background_tasks":[]"#),
            sent(AgentState::Done, "")
        );
        assert_eq!(
            mapped("Stop", r#""last_assistant_message":"secret""#),
            sent(AgentState::Done, "")
        );
    }

    #[test]
    fn stop_with_background_tasks_is_working() {
        assert_eq!(
            mapped(
                "Stop",
                r#""background_tasks":[{"description":"npm run dev","command":"npm run dev"}]"#
            ),
            sent(AgentState::Working, "background")
        );
    }

    #[test]
    fn stop_failure_sends_error_state() {
        assert_eq!(
            mapped(
                "StopFailure",
                r#""error":"rate_limit","error_details":"429 from the API""#
            ),
            sent(AgentState::Error, "API error: rate_limit")
        );
        // the longest word on the list still fits in 32 bytes
        assert_eq!(
            mapped("StopFailure", r#""error":"authentication_failed""#),
            sent(AgentState::Error, "API error: authentication_failed")
        );
    }

    #[test]
    fn stop_failure_with_unknown_error_says_unknown() {
        assert_eq!(
            mapped("StopFailure", r#""error":"my secret text""#),
            sent(AgentState::Error, "API error: unknown")
        );
        assert_eq!(
            mapped("StopFailure", r#""error":{"code":1}"#),
            sent(AgentState::Error, "API error: unknown")
        );
        assert_eq!(
            mapped("StopFailure", ""),
            sent(AgentState::Error, "API error: unknown")
        );
    }

    #[test]
    fn session_end_is_ended() {
        assert_eq!(
            mapped("SessionEnd", r#""reason":"prompt_input_exit""#),
            sent(AgentState::Ended, "")
        );
    }

    #[test]
    fn unknown_event_sends_nothing() {
        assert_eq!(mapped("SubagentStart", ""), None);
        assert_eq!(mapped("PreCompact", ""), None);
        assert_eq!(mapped("", ""), None);
    }

    // ---- the denied-tool sequence from the T-038 spike ----

    #[test]
    fn denied_tool_sequence_ends_in_done() {
        /*
         * PreToolUse, PermissionRequest about 100 ms later, the user denies,
         * no PostToolUse or PostToolUseFailure, then Stop
         */
        let pre = mapped("PreToolUse", r#""tool_name":"Bash""#);
        let ask = mapped("PermissionRequest", r#""tool_name":"Bash""#);
        let stop = mapped("Stop", "");

        assert_eq!(pre, sent(AgentState::Working, "Bash"));
        assert_eq!(ask, sent(AgentState::NeedsYou, "allow Bash?"));
        assert_eq!(stop, sent(AgentState::Done, ""));
    }

    // ---- tool labels ----

    #[test]
    fn mcp_tool_label_is_plain_mcp() {
        let name = Some("mcp__my-private-server__read_file".to_string());
        assert_eq!(tool_label(&name), "MCP");
        assert_eq!(
            mapped(
                "PermissionRequest",
                r#""tool_name":"mcp__github__create_issue""#
            ),
            sent(AgentState::NeedsYou, "allow MCP?")
        );
    }

    #[test]
    fn agent_tool_label_is_agent() {
        assert_eq!(tool_label(&Some("Agent".to_string())), "Agent");
    }

    #[test]
    fn missing_tool_name_is_called_tool() {
        assert_eq!(tool_label(&None), "tool");
        assert_eq!(tool_label(&Some(String::new())), "tool");
    }

    #[test]
    fn tool_label_is_cut_on_a_char_boundary() {
        // 31 ASCII bytes, then a 2-byte character that would cross the 32-byte line
        let name = Some(format!("{}é-more", "T".repeat(31)));
        assert_eq!(tool_label(&name), "T".repeat(31));
    }

    #[test]
    fn tool_label_drops_control_characters() {
        let name = Some("Ba\u{1b}[2Jsh\n".to_string());
        assert_eq!(tool_label(&name), "Ba[2Jsh");
    }

    #[test]
    fn long_tool_name_keeps_the_question_mark_in_allow_text() {
        let extra = format!(r#""tool_name":"{}""#, "X".repeat(40));
        let (_, doing) = mapped("PermissionRequest", &extra).unwrap();
        assert!(doing.len() <= MAX_DOING_BYTES);
        assert!(doing.starts_with("allow X"));
        assert!(doing.ends_with('?'));
    }

    #[test]
    fn subagent_tool_call_maps_like_a_main_tool_call() {
        // inside a subagent the hook carries the parent's session_id and an agent_type
        let main_call = hook("PreToolUse", r#""tool_name":"Grep""#);
        let sub_call = hook(
            "PreToolUse",
            r#""tool_name":"Grep","agent_id":"a-1","agent_type":"general-purpose""#,
        );

        assert_eq!(map_event(&sub_call), map_event(&main_call));
        assert_eq!(map_event(&sub_call), sent(AgentState::Working, "Grep"));
        assert_eq!(
            session_sid(&sub_call.session_id),
            session_sid(&main_call.session_id)
        );
    }

    // ---- sid ----

    #[test]
    fn sid_is_stable_for_a_fixed_session_id() {
        assert_eq!(
            session_sid("0b9f8c2e-1d4a-4f6b-9a3e-5c7d2e1f0a9b"),
            "578eaed40e2a"
        );
    }

    #[test]
    fn sid_is_12_hex() {
        for session_id in ["a", "another-session", "\u{1F600}"] {
            let sid = session_sid(session_id);
            assert_eq!(sid.len(), 12);
            for c in sid.chars() {
                assert!(c.is_ascii_digit() || ('a'..='f').contains(&c));
            }
        }
        assert_ne!(session_sid("a"), session_sid("b"));
    }

    // ---- project ----

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn project_prefers_claude_project_dir_over_cwd() {
        let project = resolve_project(
            &None,
            false,
            &some("/home/user/projects/deskpane"),
            &some("/home/user/projects/deskpane/daemon/src"),
            &None,
        );
        assert_eq!(project, "deskpane");
    }

    #[test]
    fn project_falls_back_to_cwd() {
        let project = resolve_project(&None, false, &None, &some("/home/user/work/api"), &None);
        assert_eq!(project, "api");
        let empty_dir = resolve_project(&None, false, &some(""), &some("/srv/site"), &None);
        assert_eq!(empty_dir, "site");
    }

    #[test]
    fn project_is_basename_only() {
        assert_eq!(
            resolve_project(
                &None,
                false,
                &some("/home/user/projects/deskpane/"),
                &None,
                &None
            ),
            "deskpane"
        );
        assert_eq!(
            resolve_project(
                &None,
                false,
                &some("C:\\Users\\user\\code\\deskpane"),
                &None,
                &None
            ),
            "deskpane"
        );
        assert_eq!(
            resolve_project(&None, false, &some("deskpane"), &None, &None),
            "deskpane"
        );
    }

    #[test]
    fn project_alias_wins() {
        let project = resolve_project(
            &some("client work"),
            false,
            &some("/home/user/projects/acme-secret"),
            &None,
            &None,
        );
        assert_eq!(project, "client work");
    }

    #[test]
    fn hide_project_sends_empty() {
        let project = resolve_project(
            &some("alias"),
            true,
            &some("/home/user/projects/deskpane"),
            &None,
            &None,
        );
        assert_eq!(project, "");
    }

    #[test]
    fn no_project_source_sends_empty() {
        assert_eq!(resolve_project(&None, false, &None, &None, &None), "");
        assert_eq!(
            resolve_project(&some(""), false, &some("/"), &None, &None),
            ""
        );
    }

    #[test]
    fn project_is_cut_to_32_bytes_without_control_characters() {
        let long = format!("/p/{}", "n".repeat(40));
        assert_eq!(
            resolve_project(&None, false, &some(&long), &None, &None),
            "n".repeat(32)
        );
        assert_eq!(
            resolve_project(&some("a\tb\u{7f}c"), false, &None, &None, &None),
            "abc"
        );
    }

    // ---- packet ----

    #[test]
    fn built_packet_passes_the_daemon_check() {
        let packet = build_agent_packet(
            "578eaed40e2a",
            "dp-3f9a0c41",
            "deskpane",
            AgentState::NeedsYou,
            "allow Bash?",
            1794152400123,
            None,
        );

        assert_eq!(validate_agent(&packet), Ok(()));
        assert_eq!(
            serde_json::to_string(&packet).unwrap(),
            r#"{"kind":"agent","v":1,"sid":"578eaed40e2a","device_id":"dp-3f9a0c41","project":"deskpane","state":"needs_you","doing":"allow Bash?","t":1794152400123}"#
        );
    }

    // ---- review fixes (T-045) ----

    #[test]
    fn project_in_the_home_folder_sends_empty() {
        let home = some("/home/user");
        assert_eq!(
            resolve_project(&None, false, &some("/home/user"), &None, &home),
            ""
        );
        assert_eq!(
            resolve_project(&None, false, &some("/home/user/"), &None, &home),
            ""
        );
        assert_eq!(
            resolve_project(&None, false, &None, &some("/home/user"), &home),
            ""
        );
        let mac = some("/Users/user/");
        assert_eq!(
            resolve_project(&None, false, &some("/Users/user"), &None, &mac),
            ""
        );
        let windows = some("C:\\Users\\user");
        assert_eq!(
            resolve_project(&None, false, &some("C:\\Users\\user\\"), &None, &windows),
            ""
        );
        assert_eq!(
            resolve_project(&None, false, &some("c:/users/user"), &None, &windows),
            ""
        );
    }

    #[test]
    fn project_below_the_home_folder_is_still_sent() {
        let home = some("/home/user");
        assert_eq!(
            resolve_project(&None, false, &some("/home/user/deskpane"), &None, &home),
            "deskpane"
        );
        // an alias always wins, even in the home folder
        assert_eq!(
            resolve_project(&some("notes"), false, &some("/home/user"), &None, &home),
            "notes"
        );
        // no home known: nothing to compare with
        assert_eq!(
            resolve_project(&None, false, &some("/home/user"), &None, &None),
            "user"
        );
        assert_eq!(
            resolve_project(&None, false, &some("/home/user"), &None, &some("")),
            "user"
        );
    }

    #[test]
    fn subagent_hook_carries_a_hashed_sub() {
        let input = hook(
            "PreToolUse",
            r#""tool_name":"Grep","agent_id":"a-1","agent_type":"general-purpose""#,
        );

        let sub = sub_key(&input.agent_id);

        assert_eq!(sub, Some("8e9f65ad".to_string()));
        let packet = build_agent_packet(
            "578eaed40e2a",
            "my-pc",
            "deskpane",
            AgentState::Working,
            "Grep",
            1,
            sub,
        );
        assert_eq!(validate_agent(&packet), Ok(()));
        let json = serde_json::to_string(&packet).unwrap();
        assert!(json.contains(r#""sub":"8e9f65ad""#));
        assert!(!json.contains("a-1"));
    }

    #[test]
    fn main_agent_hook_has_no_sub() {
        let input = hook("PreToolUse", r#""tool_name":"Grep""#);
        assert_eq!(sub_key(&input.agent_id), None);
        // an empty or non-text agent_id counts as none
        assert_eq!(sub_key(&Some(serde_json::json!(""))), None);
        assert_eq!(sub_key(&Some(serde_json::json!(5))), None);
        let packet = build_agent_packet(
            "578eaed40e2a",
            "my-pc",
            "",
            AgentState::Working,
            "Grep",
            1,
            None,
        );
        assert!(!serde_json::to_string(&packet).unwrap().contains("sub"));
    }

    #[test]
    fn format_characters_are_dropped_from_project_and_tool_label() {
        assert_eq!(
            resolve_project(
                &some("desk\u{202E}pane\u{200B}"),
                false,
                &None,
                &None,
                &None
            ),
            "deskpane"
        );
        assert_eq!(tool_label(&Some("Ba\u{2066}sh".to_string())), "Bash");
    }
}
