#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events},
    token, vec, Address, Env, String, Symbol, TryFromVal,
};

fn setup_whitelist_test(
    env: &Env,
    initial_amount: i128,
) -> (
    ProgramEscrowContractClient<'static>,
    Address,
    token::Client<'static>,
    token::StellarAssetClient<'static>,
) {
    env.mock_all_auths();

    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let tokenadmin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(tokenadmin.clone());
    let token_client = token::Client::new(env, &token_id);
    let tokenadmin_client = token::StellarAssetClient::new(env, &token_id);

    // Initialize contract
    client.initialize_contract(&admin);

    // Initialize program
    let program_id = String::from_str(env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);

    tokenadmin_client.mint(&admin, &1_000_000_000);
    if initial_amount > 0 {
        tokenadmin_client.mint(&admin, &initial_amount);
        client.lock_program_funds(&admin, &initial_amount);
    }

    (client, admin, token_client, tokenadmin_client)
}

fn find_event_by_topic(
    env: &Env,
    topic: Symbol,
) -> Option<(soroban_sdk::Vec<soroban_sdk::Val>, soroban_sdk::Val)> {
    let events = env.events().all();
    for i in 0..events.len() {
        let event = events.get(i).unwrap();
        let topics = event.1;
        if topics.len() > 0 {
            let first_topic = topics.get(0).unwrap();
            if let Ok(sym) = Symbol::try_from_val(env, &first_topic) {
                if sym == topic {
                    return Some((topics, event.2));
                }
            }
        }
    }
    None
}

#[test]
fn test_set_and_unset_whitelist() {
    let env = Env::default();
    let (client, admin, _, _) = setup_whitelist_test(&env, 0);

    let addr1 = Address::generate(&env);
    let addr2 = Address::generate(&env);

    // Default: not whitelisted
    assert!(!client.is_whitelisted(&addr1));
    assert!(!client.is_whitelisted(&addr2));

    // Admin sets whitelist to true
    client.set_whitelist(&addr1, &true);
    assert!(client.is_whitelisted(&addr1));
    assert!(!client.is_whitelisted(&addr2));

    // Verify event emission
    let event = find_event_by_topic(&env, symbol_short!("WlChange"));
    assert!(event.is_some());
    let (_, data) = event.unwrap();
    let decoded: WhitelistChangedEvent = WhitelistChangedEvent::try_from_val(&env, &data).unwrap();
    assert_eq!(decoded.address, addr1);
    assert!(decoded.whitelisted);

    // Admin sets whitelist to false
    client.set_whitelist(&addr1, &false);
    assert!(!client.is_whitelisted(&addr1));
}

#[test]
#[should_panic]
fn test_set_whitelist_requires_admin_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize_contract(&admin);

    let addr = Address::generate(&env);
    // Since mock_all_auths is not set, this should panic because admin.require_auth() fails
    client.set_whitelist(&addr, &true);
}

#[test]
fn test_set_and_unset_whitelist_enforcement() {
    let env = Env::default();
    let (client, admin, _, _) = setup_whitelist_test(&env, 0);

    // Default: false
    assert!(!client.is_whitelist_enforced());

    // Admin sets enforced to true
    client.set_whitelist_enforced(&true);
    assert!(client.is_whitelist_enforced());

    // Verify event emission
    let event = find_event_by_topic(&env, symbol_short!("WlEnfChg"));
    assert!(event.is_some());
    let (_, data) = event.unwrap();
    let decoded: WhitelistEnforcementChangedEvent =
        WhitelistEnforcementChangedEvent::try_from_val(&env, &data).unwrap();
    assert!(decoded.enabled);

    // Admin sets enforced to false
    client.set_whitelist_enforced(&false);
    assert!(!client.is_whitelist_enforced());
}

#[test]
#[should_panic]
fn test_set_whitelist_enforced_requires_admin_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize_contract(&admin);

    // This should panic
    client.set_whitelist_enforced(&true);
}

#[test]
fn test_whitelist_enforcement_off_single_payout_succeeds() {
    let env = Env::default();
    let (client, admin, token_client, _) = setup_whitelist_test(&env, 100_000);
    let recipient = Address::generate(&env);

    // Whitelist enforcement is OFF (default).
    // Non-whitelisted address can receive payouts.
    client.single_payout(&recipient, &10_000);
    assert_eq!(token_client.balance(&recipient), 10_000);
}

#[test]
#[should_panic(expected = "Recipient not whitelisted")]
fn test_single_payout_with_enforcement_non_whitelisted_panics() {
    let env = Env::default();
    let (client, admin, _, _) = setup_whitelist_test(&env, 100_000);
    let recipient = Address::generate(&env);

    client.set_whitelist_enforced(&true);
    // Non-whitelisted recipient -> should panic
    client.single_payout(&recipient, &10_000);
}

#[test]
fn test_single_payout_with_enforcement_whitelisted_succeeds() {
    let env = Env::default();
    let (client, admin, token_client, _) = setup_whitelist_test(&env, 100_000);
    let recipient = Address::generate(&env);

    client.set_whitelist_enforced(&true);
    client.set_whitelist(&recipient, &true);

    // Whitelisted recipient -> succeeds
    client.single_payout(&recipient, &10_000);
    assert_eq!(token_client.balance(&recipient), 10_000);
}

#[test]
#[should_panic(expected = "Recipient not whitelisted")]
fn test_batch_payout_with_enforcement_non_whitelisted_panics() {
    let env = Env::default();
    let (client, admin, _, _) = setup_whitelist_test(&env, 100_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);

    // Whitelist only r1, not r2
    client.set_whitelist(&r1, &true);
    client.set_whitelist_enforced(&true);

    let recipients = vec![&env, r1, r2];
    let amounts = vec![&env, 10_000, 15_000];

    // Batch payout has a non-whitelisted address -> should panic
    client.batch_payout(&recipients, &amounts);
}

#[test]
fn test_batch_payout_with_enforcement_whitelisted_succeeds() {
    let env = Env::default();
    let (client, admin, token_client, _) = setup_whitelist_test(&env, 100_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);

    client.set_whitelist(&r1, &true);
    client.set_whitelist(&r2, &true);
    client.set_whitelist_enforced(&true);

    let recipients = vec![&env, r1.clone(), r2.clone()];
    let amounts = vec![&env, 10_000, 15_000];

    client.batch_payout(&recipients, &amounts);
    assert_eq!(token_client.balance(&r1), 10_000);
    assert_eq!(token_client.balance(&r2), 15_000);
}

#[test]
fn test_batch_payout_enforcement_off_succeeds() {
    let env = Env::default();
    let (client, admin, token_client, _) = setup_whitelist_test(&env, 100_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);

    // Whitelist is enforced = false (default)
    let recipients = vec![&env, r1.clone(), r2.clone()];
    let amounts = vec![&env, 10_000, 15_000];

    client.batch_payout(&recipients, &amounts);
    assert_eq!(token_client.balance(&r1), 10_000);
    assert_eq!(token_client.balance(&r2), 15_000);
}

#[test]
fn test_whitelist_atomicity_prevents_mid_flight_removal_race() {
    let env = Env::default();
    let (client, _admin, token_client, _) = setup_whitelist_test(&env, 100_000);
    let recipient = Address::generate(&env);

    client.set_whitelist_enforced(&true);
    
    // DOCUMENTATION OF STRUCTURAL IMPOSSIBILITY:
    // This test serves as a regression test asserting that whitelist-gated
    // actions (like single_payout and batch_payout) are atomic, check-then-act 
    // operations. There are no multi-step or multi-call flows spanning multiple
    // contract invocations that are gated by the whitelist.
    // 
    // Because the whitelist check and the payout happen synchronously within 
    // the same invocation, it is structurally impossible for an admin to remove
    // a party from the whitelist "mid-flight" between steps of a flow.
    // Either the party is whitelisted at the exact moment of the payout call 
    // and receives funds, or they are not, and the transaction reverts.

    // State 1: Recipient is not whitelisted. Atomic call fails immediately.
    let res = client.try_single_payout(&recipient, &10_000);
    assert!(res.is_err());
    assert_eq!(token_client.balance(&recipient), 0);

    // State 2: Admin adds recipient to whitelist.
    client.set_whitelist(&recipient, &true);
    
    // Now the atomic call succeeds in a single step.
    client.single_payout(&recipient, &10_000);
    assert_eq!(token_client.balance(&recipient), 10_000);

    // State 3: Admin removes recipient from whitelist.
    // There is no "in-progress" flow to resume. Future atomic calls simply fail.
    client.set_whitelist(&recipient, &false);
    
    let res2 = client.try_single_payout(&recipient, &10_000);
    assert!(res2.is_err());
    assert_eq!(token_client.balance(&recipient), 10_000); // Balance unchanged
}
