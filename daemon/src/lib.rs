pub mod agent_table;
pub mod device_table;
pub mod metric_table;
pub mod services;

// the wire contract now lives in its own crate (deskpane-contract) so the
// reporter and future plugins can depend on it without pulling in the daemon
// and tokio. re-exported here as `contract` so every `daemon::contract::...`
// and `crate::contract::...` path keeps working unchanged.
pub use deskpane_contract as contract;
