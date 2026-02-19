//! Trigger system for FlowEngine v2.
//!
//! Triggers answer "when should this flow run?" — HTTP webhooks, cron
//! schedules, and flow-chaining are built-in. The [`TriggerRunner`] manages
//! trigger lifecycles, and the [`TriggerDispatcher`] resolves incoming
//! events to versioned flows for execution.

mod cron_trigger;
mod dispatcher;
mod flow_trigger;
mod http_trigger;
mod runner;

pub use cron_trigger::CronTrigger;
pub use dispatcher::{DispatchedRun, TriggerDispatcher};
pub use flow_trigger::{FlowTrigger, RunCompletedEvent};
pub use http_trigger::{HttpTrigger, HttpTriggerRoute};
pub use runner::{TriggerInstance, TriggerRunner};
