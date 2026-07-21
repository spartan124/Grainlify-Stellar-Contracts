#![cfg(test)]

use crate::{ProgramEscrowContract, ProgramEscrowContractClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events, Ledger},
    token, Address, Env, Map, String, Symbol, TryFromVal, Val,
};

#[test]
fn test_monitoring_analytics_and_health() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(&env, &contract_id);

    // Initial health check
    let initial_health = client.health_check();
    assert_eq!(initial_health.is_healthy, true);
    assert_eq!(initial_health.total_operations, 0);

    let backend = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_sac = soroban_sdk::token::StellarAssetClient::new(&env, &token);
    let prog_id = String::from_str(&env, "TestHealth");

    // Test init metric
    env.ledger().set_timestamp(100);
    client.init_program(&prog_id, &backend, &token);

    let analytics = client.get_monitoring_analytics();
    assert_eq!(analytics.operation_count, 1);
    assert_eq!(analytics.error_count, 0);

    let stats = client.get_performance_stats(&symbol_short!("init"));
    assert_eq!(stats.call_count, 1);

    // Test lock metric
    env.ledger().set_timestamp(200);
    let admin = Address::generate(&env);
    token_sac.mint(&admin, &5000);
    client.lock_program_funds(&admin, &5000);

    let analytics2 = client.get_monitoring_analytics();
    assert_eq!(analytics2.operation_count, 2);

    // Test lock error metric (trigger panic)
    let admin = Address::generate(&env);
    let result = client.try_lock_program_funds(&admin, &0);
    assert!(result.is_err());

    let analytics3 = client.get_monitoring_analytics();
    // Two successful operations tracked; error doesn't add to operation_count
    assert_eq!(analytics3.operation_count, 2);
    // Error count reflects the error_count stored from monitoring context
    assert_eq!(analytics3.error_count, 0);

    // Test state snapshot
    let snapshot = client.get_state_snapshot();
    assert_eq!(snapshot.total_operations, 2);
    assert_eq!(snapshot.total_errors, 0);
}

#[test]
fn test_default_large_payout_threshold_is_ten_percent() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.setadmin(&admin);

    let threshold_bps = client.get_large_payout_threshold();
    assert_eq!(threshold_bps, 1000);
}

#[test]
fn test_admin_can_update_large_payout_threshold() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.setadmin(&admin);

    assert_eq!(client.get_large_payout_threshold(), 1000);
    client.try_set_large_payout_threshold(&2000).unwrap();
    assert_eq!(client.get_large_payout_threshold(), 2000);
}

#[test]
#[should_panic]
fn test_non_admin_cannot_update_large_payout_threshold() {
    let env = Env::default();

    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.setadmin(&admin);

    client.try_set_large_payout_threshold(&2000).unwrap();
}

// --- Alert-threshold breach tests for monitoring.rs (issue #192) ---
//
// The only monitored breach metric implemented in monitoring.rs is the
// "large payout" detector: `check_and_emit_large_payout` emits a `(LrgPay,)`
// event when `amount >= threshold`, where
// `threshold = total_funds * large_payout_threshold_bps / 10_000`
// (default bps = 1000 => 10% of locked funds).
//
// There is NO rolling-window / rapid-successive-claims tracking in
// monitoring.rs, so the rolling-window acceptance criterion is N/A (asserted
// below as a documented skip, not a test).

fn setup_with_funds(
    env: &Env,
    total_funds: i128,
) -> (ProgramEscrowContractClient<'static>, Address, token::StellarAssetClient<'static>) {
    env.mock_all_auths();
    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let tokenadmin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(tokenadmin.clone());
    let tokenadmin_client = soroban_sdk::token::StellarAssetClient::new(env, &token_id);

    let program_id = String::from_str(env, "threshold-test");
    client.init_program(&program_id, &admin, &token_id);
    tokenadmin_client.mint(&admin, &total_funds);
    client.lock_program_funds(&admin, &total_funds);

    (client, admin, tokenadmin_client)
}

fn large_payout_event(env: &Env) -> Option<Val> {
    let events = env.events().all();
    for i in 0..events.len() {
        let event = events.get(i).unwrap();
        let topics = event.1;
        if topics.len() > 0 {
            if let Ok(sym) = Symbol::try_from_val(env, &topics.get(0).unwrap()) {
                if sym == symbol_short!("LrgPay") {
                    return Some(event.2);
                }
            }
        }
    }
    None
}

#[test]
fn test_large_payout_no_event_just_below_threshold() {
    let env = Env::default();
    // 100_000 locked => default threshold (10%) = 10_000.
    let (client, _admin, _token) = setup_with_funds(&env, 100_000);

    let recipient = Address::generate(&env);
    client.single_payout(&recipient, &9_999); // just below 10_000

    assert!(
        large_payout_event(&env).is_none(),
        "LrgPay event must NOT fire below threshold"
    );
}

#[test]
fn test_large_payout_event_fires_exactly_at_threshold() {
    let env = Env::default();
    let total_funds: i128 = 100_000;
    let (client, _admin, _token) = setup_with_funds(&env, total_funds);

    let recipient = Address::generate(&env);
    client.single_payout(&recipient, &10_000); // exactly at 10% threshold

    let data = large_payout_event(&env).expect("LrgPay event must fire at threshold");
    let map: soroban_sdk::Map<Symbol, Val> = soroban_sdk::Map::try_from_val(&env, &data).unwrap();
    let amount = <i128 as soroban_sdk::TryFromVal<Env, Val>>::try_from_val(
        &env,
        &map.get(Symbol::new(&env, "amount")).unwrap(),
    )
    .unwrap();
    let threshold = <i128 as soroban_sdk::TryFromVal<Env, Val>>::try_from_val(
        &env,
        &map.get(Symbol::new(&env, "threshold")).unwrap(),
    )
    .unwrap();
    assert_eq!(amount, 10_000);
    assert_eq!(threshold, 10_000); // 100_000 * 1000 / 10_000
}

#[test]
fn test_large_payout_event_fires_above_threshold() {
    let env = Env::default();
    let (client, _admin, _token) = setup_with_funds(&env, 100_000);

    let recipient = Address::generate(&env);
    client.single_payout(&recipient, &10_001); // just above threshold

    assert!(
        large_payout_event(&env).is_some(),
        "LrgPay event must fire above threshold"
    );
}

#[test]
fn test_large_payout_threshold_respects_custom_bps() {
    let env = Env::default();
    // Override threshold to 25% (2500 bps) => 25_000 on 100_000 funds.
    let (client, admin, _token) = setup_with_funds(&env, 100_000);
    client.setadmin(&admin);
    client.try_set_large_payout_threshold(&2500).unwrap();

    let recipient = Address::generate(&env);
    client.single_payout(&recipient, &24_999); // below 25_000
    assert!(
        large_payout_event(&env).is_none(),
        "no event below custom 25% threshold"
    );

    let recipient2 = Address::generate(&env);
    client.single_payout(&recipient2, &25_000); // exactly at 25% threshold
    let data = large_payout_event(&env).expect("event must fire at custom threshold");
    let map: soroban_sdk::Map<Symbol, Val> = soroban_sdk::Map::try_from_val(&env, &data).unwrap();
    let threshold = <i128 as soroban_sdk::TryFromVal<Env, Val>>::try_from_val(
        &env,
        &map.get(Symbol::new(&env, "threshold")).unwrap(),
    )
    .unwrap();
    assert_eq!(threshold, 25_000);
}

// Rolling-window / metric decay and alert clearing tests (issue #238)
#[test]
fn test_metric_decay_and_alert_clearing() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(&env, &contract_id);

    // Initial state: healthy
    assert_eq!(client.health_check().is_healthy, true);

    // Setup program for operations
    let admin = Address::generate(&env);
    let tokenadmin = Address::generate(&env);
    let token_id = env.register_stellar_asset_contract(tokenadmin.clone());
    let program_id = String::from_str(&env, "decay-test");
    
    // Set initial time
    env.ledger().set_timestamp(1000);
    
    // Operation 1 (Init): success
    client.init_program(&program_id, &admin, &token_id);
    assert_eq!(client.health_check().is_healthy, true);
    
    // Operation 2: Trigger error
    // Since normal contract panics roll back the state, we directly inject a tracked failure
    // as if a non-panicking internal check failed and tracked it.
    env.as_contract(&contract_id, || {
        crate::monitoring::track_operation(&env, symbol_short!("lock"), admin.clone(), false);
    });
    
    // At this point in the window: 1 success, 1 failure -> 50% error rate
    // Threshold is 50%, so it should trip the alert (is_healthy = false)
    assert_eq!(client.health_check().is_healthy, false);
    
    let snap1 = client.get_state_snapshot();
    assert_eq!(snap1.total_operations, 2);
    assert_eq!(snap1.total_errors, 1);
    
    // Edge case: Metric right at the decay boundary (1 hour = 3600 seconds)
    // At timestamp 1000 + 3599 = 4599, the window has NOT decayed
    env.ledger().set_timestamp(4599);
    assert_eq!(client.health_check().is_healthy, false);
    
    // Metric decays (window reset) at boundary: timestamp 1000 + 3600 = 4600
    env.ledger().set_timestamp(4600);
    
    // Alert state clears due to decay of old metrics
    assert_eq!(client.health_check().is_healthy, true);
    
    // The metric that should NEVER decay (ERROR_COUNT) is confirmed not to
    let snap2 = client.get_state_snapshot();
    assert_eq!(snap2.total_operations, 2);
    assert_eq!(snap2.total_errors, 1);
    
    // Perform a new operation in the new window (success)
    env.as_contract(&contract_id, || {
        crate::monitoring::track_operation(&env, symbol_short!("lock"), admin.clone(), true);
    });
    
    // Window now has 1 success, 0 errors -> 0% error rate
    assert_eq!(client.health_check().is_healthy, true);
    
    // Overall metrics continue to accumulate
    let snap3 = client.get_state_snapshot();
    assert_eq!(snap3.total_operations, 3);
    assert_eq!(snap3.total_errors, 1);
}


