//! With the steward setting off, the runtime makes no extra requests and the
//! main request stream is identical to a run with no steward at all.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use agent_runtime::{
    run_loop,
    testing::MockProvider,
    CancelToken, JsonMode, Ledger, LedgerEntry, ModelProfile, PinAuthor, PinKind, Steward,
    StewardConfig, StewardSink,
};

use crate::common::{config, render, sink};

fn run(steward: Option<Arc<Steward>>, provider: Arc<MockProvider>) {
    let mut run_config = config(provider, ModelProfile::new(32_000, true));
    run_config.steward = steward;
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.add_pin(PinKind::Requirement, "Be brief.", PinAuthor::User);
    ledger.push(LedgerEntry::user("hello"));
    let (sink, _events) = sink();
    run_loop(run_config, ledger, CancelToken::new(), sink);
}

#[test]
fn a_disabled_steward_costs_nothing() {
    let main = MockProvider::constant("answer");
    let baseline = MockProvider::constant("answer");
    let steward_provider = MockProvider::constant("{}");
    let events = Arc::new(Mutex::new(0usize));
    let steward_sink: StewardSink = {
        let events = events.clone();
        Arc::new(move |_| *events.lock().unwrap() += 1)
    };
    let steward = Steward::new(
        StewardConfig {
            provider: steward_provider.clone(),
            model: "steward".to_string(),
            enabled: false,
            drift_check: true,
            json_mode: JsonMode::Schema,
        },
        steward_sink,
    );
    assert!(!steward.is_enabled());

    run(Some(steward.clone()), main.clone());
    run(None, baseline.clone());
    std::thread::sleep(Duration::from_millis(150));

    assert_eq!(main.request_count(), 1, "exactly the user's request");
    assert_eq!(steward_provider.request_count(), 0);
    assert_eq!(steward.requests_made(), 0);
    assert_eq!(*events.lock().unwrap(), 0);
    // Same bytes on the wire as a run without any steward.
    assert_eq!(
        render(&main.requests()[0].messages),
        render(&baseline.requests()[0].messages)
    );
}
