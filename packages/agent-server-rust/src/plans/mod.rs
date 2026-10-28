pub mod auth_status;
pub mod chat_open;
mod composer;
pub mod login;
pub mod logout;
pub mod send_message;

use crate::ia::types::{AppState, IdentifiedStates, SelectedAction, A11yNode};

/// Plan trait — defines a goal-oriented sequence of actions.
///
/// Plans access the database via `crate::db::get_db()` internally rather than
/// receiving a `&Connection` parameter, because rusqlite's `Connection` is not
/// `Sync` and thus cannot be held across await points in async trait methods.
#[async_trait::async_trait]
pub trait Plan: Send + Sync {
    type PlanState: Send;
    type Params: Send;

    fn id(&self) -> &str;

    fn initial_plan_state(&self) -> Self::PlanState;

    fn is_goal_reached(
        &self,
        state: &AppState,
        plan_state: &Self::PlanState,
    ) -> bool;

    async fn select_action(
        &self,
        state: &AppState,
        params: &Self::Params,
        identified: &IdentifiedStates,
        plan_state: &mut Self::PlanState,
        a11y: &A11yNode,
        session_id: &str,
    ) -> Option<SelectedAction>;

    /// Explain why `select_action` returned `None`, when the plan knows.
    ///
    /// Returning `None` from `select_action` means "no applicable action", which
    /// the execution loop reports as "No action selected". A plan that hits a
    /// recognised blocker (for example a composer that cannot accept text) can
    /// describe it here so callers see the real reason instead of a generic
    /// error.
    fn failure_reason(&self, _plan_state: &Self::PlanState) -> Option<String> {
        None
    }
}
pub mod chat_cache;
