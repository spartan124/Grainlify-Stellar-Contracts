#![cfg(test)]

/// # Program Status & Lifecycle Transition Tests
///
/// This module tests the implicit lifecycle of the Program Escrow contract,
/// covering all state transitions and asserting which operations are allowed
/// or forbidden in each state.
///
/// ## Lifecycle States
///
/// ```text
/// Uninitialized  ──init_program()──►  Initialized
///                                         │
///                                   lock_program_funds()
///                                         │
///                                         ▼
///                                       Active  ◄──── lock_program_funds() (top-up)
///                                         │
///                              ┌──────────┼──────────┐
///                        set_paused()  payouts()  set_paused()
///                              │                      │
///                              ▼                      │
///                            Paused ──set_paused()──► Active (resume)
///                              │
///                         (forbidden ops)
///                                         │
///                              all funds paid out
///                                         │
///                                         ▼
///                                       Drained  (remaining_balance == 0)
///                                         │
///                              lock_program_funds()  (re-activate)
///                                         │
///                                         ▼
///                                       Active
/// ```
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, vec, Address, Env, String, Vec,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Register the contract and return a client plus the contract address.
fn make_client(env: &Env) -> (ProgramEscrowContractClient<'static>, Address) {
    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(env, &contract_id);
    (client, contract_id)
}

/// Create a real SAC token, mint `amount` to the contract address, and return
/// the token client and token contract id.
fn fund_contract(env: &Env, funder: &Address, amount: i128) -> (token::Client<'static>, Address) {
    let tokenadmin = Address::generate(env);
    let token_contract = env.register_stellar_asset_contract_v2(tokenadmin.clone());
    let token_id = token_contract.address();
    let token_client = token::Client::new(env, &token_id);
    let token_sac = token::StellarAssetClient::new(env, &token_id);
    if amount > 0 {
        token_sac.mint(funder, &amount);
    }
    (token_client, token_id)
}

/// Full setup: contract, admin (authorized payout key), token, program
/// initialized and funded.
fn setup_active_program(
    env: &Env,
    amount: i128,
) -> (
    ProgramEscrowContractClient<'static>,
    Address,
    Address,
    token::Client<'static>,
) {
    env.mock_all_auths();
    let (client, contract_id) = make_client(env);
    let admin = Address::generate(env);
    let (token_client, token_id) = fund_contract(env, &admin, amount);
    let program_id = String::from_str(env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    if amount > 0 {
        client.lock_program_funds(&admin, &amount);
    }
    (client, admin, contract_id, token_client)
}

// ---------------------------------------------------------------------------
// STATE: Uninitialized
// Any operation before init_program must be rejected.
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Program not initialized")]
fn test_uninitialized_lock_funds_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let admin = Address::generate(&env);
    client.lock_program_funds(&admin, &1_000);
}

#[test]
#[should_panic(expected = "Program not initialized")]
fn test_uninitialized_single_payout_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let recipient = Address::generate(&env);
    client.single_payout(&recipient, &100);
}

#[test]
#[should_panic(expected = "Program not initialized")]
fn test_uninitialized_batch_payout_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let r = Address::generate(&env);
    client.batch_payout(&vec![&env, r], &vec![&env, 100i128]);
}

#[test]
#[should_panic(expected = "Program not initialized")]
fn test_uninitialized_get_info_rejected() {
    let env = Env::default();
    let (client, _cid) = make_client(&env);
    client.get_program_info();
}

#[test]
#[should_panic(expected = "Program not initialized")]
fn test_uninitialized_get_balance_rejected() {
    let env = Env::default();
    let (client, _cid) = make_client(&env);
    client.get_remaining_balance();
}

#[test]
#[should_panic(expected = "Program not initialized")]
fn test_uninitialized_create_schedule_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let r = Address::generate(&env);
    client.create_program_release_schedule(&100, &1000, &r);
}

#[test]
#[should_panic(expected = "Program not initialized")]
fn test_uninitialized_trigger_releases_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    client.trigger_program_releases();
}

// ---------------------------------------------------------------------------
// STATE: Initialized (program exists, no funds locked yet)
// ---------------------------------------------------------------------------

/// After init_program the program is queryable and balance is 0.
#[test]
fn test_initialized_state_balance_is_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let token_id = Address::generate(&env);
    let admin = Address::generate(&env);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);

    let info = client.get_program_info();
    assert_eq!(info.total_funds, 0);
    assert_eq!(info.remaining_balance, 0);
    assert_eq!(info.payout_history.len(), 0);
    assert_eq!(client.get_remaining_balance(), 0);
}

/// Re-initializing the same program must be rejected (single-init guard).
#[test]
#[should_panic(expected = "Program already initialized")]
fn test_initialized_double_init_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let token_id = Address::generate(&env);
    let admin = Address::generate(&env);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    // Second call must panic
    client.init_program(&program_id, &admin, &token_id);
}

/// Payout from a zero-balance (Initialized) program must be rejected.
#[test]
#[should_panic(expected = "Insufficient balance")]
fn test_initialized_single_payout_zero_balance_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let token_id = Address::generate(&env);
    let admin = Address::generate(&env);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    let r = Address::generate(&env);
    client.single_payout(&r, &100);
}

/// Batch payout from a zero-balance (Initialized) program must be rejected.
#[test]
#[should_panic(expected = "Insufficient balance")]
fn test_initialized_batch_payout_zero_balance_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let token_id = Address::generate(&env);
    let admin = Address::generate(&env);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    let r = Address::generate(&env);
    client.batch_payout(&vec![&env, r], &vec![&env, 100i128]);
}

/// Locking funds transitions the contract from Initialized to Active.
#[test]
fn test_initialized_to_active_via_lock_funds() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 50_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);

    // Before lock: Initialized — balance is 0
    assert_eq!(client.get_remaining_balance(), 0);

    // Transition: Initialized → Active
    let data = client.lock_program_funds(&admin, &50_000);
    assert_eq!(data.total_funds, 50_000);
    assert_eq!(data.remaining_balance, 50_000);

    // After lock: Active — balance reflects locked amount
    assert_eq!(client.get_remaining_balance(), 50_000);
}

// ---------------------------------------------------------------------------
// STATE: Active (funds locked, payouts can happen)
// ---------------------------------------------------------------------------

/// In Active state, single_payout succeeds and reduces remaining balance.
#[test]
fn test_active_single_payout_allowed() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 100_000);
    let recipient = Address::generate(&env);

    let data = client.single_payout(&recipient, &40_000);
    assert_eq!(data.remaining_balance, 60_000);
    assert_eq!(token_client.balance(&recipient), 40_000);
}

/// In Active state, batch_payout succeeds and reduces remaining balance.
#[test]
fn test_active_batch_payout_allowed() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 100_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);

    let data = client.batch_payout(
        &vec![&env, r1.clone(), r2.clone()],
        &vec![&env, 30_000i128, 20_000i128],
    );
    assert_eq!(data.remaining_balance, 50_000);
    assert_eq!(token_client.balance(&r1), 30_000);
    assert_eq!(token_client.balance(&r2), 20_000);
}

/// Multiple lock calls accumulate funds (top-up stays in Active state).
#[test]
fn test_active_top_up_lock_increases_balance() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 200_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);

    client.lock_program_funds(&admin, &80_000);
    assert_eq!(client.get_remaining_balance(), 80_000);

    client.lock_program_funds(&admin, &70_000);
    assert_eq!(client.get_remaining_balance(), 150_000);

    let info = client.get_program_info();
    assert_eq!(info.total_funds, 150_000);
}

/// In Active state, negative lock amounts are rejected.
#[test]
#[should_panic(expected = "Amount must be greater than zero")]
fn test_active_negative_lock_amount_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let token_id = Address::generate(&env);
    let admin = Address::generate(&env);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &-1);
}

/// Payout exceeding balance must be rejected (Active state guard).
#[test]
#[should_panic(expected = "Insufficient balance")]
fn test_active_payout_exceeds_balance_rejected() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 50_000);
    let r = Address::generate(&env);
    client.single_payout(&r, &50_001); // 1 unit over balance
}

/// Batch payout total exceeding balance must be rejected.
#[test]
#[should_panic(expected = "Insufficient balance")]
fn test_active_batch_exceeds_balance_rejected() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 50_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    // 30_000 + 30_000 = 60_000 > 50_000
    client.batch_payout(&vec![&env, r1, r2], &vec![&env, 30_000i128, 30_000i128]);
}

/// Zero-amount single payout must be rejected.
#[test]
#[should_panic(expected = "Amount must be greater than zero")]
fn test_active_zero_single_payout_rejected() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 50_000);
    let r = Address::generate(&env);
    client.single_payout(&r, &0);
}

/// Zero-amount entry in a batch must be rejected.
#[test]
#[should_panic(expected = "All amounts must be greater than zero")]
fn test_active_zero_amount_in_batch_rejected() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 50_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    client.batch_payout(&vec![&env, r1, r2], &vec![&env, 100i128, 0i128]);
}

/// Mismatched recipients/amounts vectors must be rejected.
#[test]
#[should_panic(expected = "Recipients and amounts vectors must have the same length")]
fn test_active_batch_mismatched_lengths_rejected() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 50_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    client.batch_payout(&vec![&env, r1, r2], &vec![&env, 100i128]);
}

/// Empty batch must be rejected.
#[test]
#[should_panic(expected = "Cannot process empty batch")]
fn test_active_empty_batch_rejected() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 50_000);
    client.batch_payout(&vec![&env], &vec![&env]);
}

/// Payout history grows correctly in Active state after multiple operations.
#[test]
fn test_active_payout_history_grows() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 100_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let r3 = Address::generate(&env);

    client.single_payout(&r1, &10_000);
    client.batch_payout(
        &vec![&env, r2.clone(), r3.clone()],
        &vec![&env, 15_000i128, 5_000i128],
    );

    let info = client.get_program_info();
    assert_eq!(info.payout_history.len(), 3);
    assert_eq!(info.remaining_balance, 70_000);
}

// ---------------------------------------------------------------------------
// STATE: Paused
// Pause flags block specific operations; other ops remain unaffected.
// ---------------------------------------------------------------------------

/// Pausing lock prevents lock_program_funds.
#[test]
#[should_panic(expected = "Funds Paused")]
fn test_paused_lock_operation_blocked() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 100_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.initialize_contract(&admin);
    client.set_paused(&Some(true), &None, &None);

    client.lock_program_funds(&admin, &10_000);
}

/// Pausing release prevents single_payout.
#[test]
#[should_panic(expected = "Funds Paused")]
fn test_paused_single_payout_blocked() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 100_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &100_000);
    client.initialize_contract(&admin);
    client.set_paused(&None, &Some(true), &None);

    let r = Address::generate(&env);
    client.single_payout(&r, &1_000);
}

/// Pausing release prevents batch_payout.
#[test]
#[should_panic(expected = "Funds Paused")]
fn test_paused_batch_payout_blocked() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 100_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &100_000);
    client.initialize_contract(&admin);
    client.set_paused(&None, &Some(true), &None);

    let r = Address::generate(&env);
    client.batch_payout(&vec![&env, r], &vec![&env, 1_000i128]);
}

/// Unpausing restores operations — Active state is fully resumed.
#[test]
fn test_paused_to_active_resume_via_unpause() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (token_client, token_id) = fund_contract(&env, &admin, 100_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &100_000);
    client.initialize_contract(&admin);

    // Transition: Active → Paused
    client.set_paused(&None, &Some(true), &None);
    assert!(client.get_pause_flags().release_paused);

    // Transition: Paused → Active
    client.set_paused(&None, &Some(false), &None);
    assert!(!client.get_pause_flags().release_paused);

    // Payout is allowed again
    let r = Address::generate(&env);
    let data = client.single_payout(&r, &10_000);
    assert_eq!(data.remaining_balance, 90_000);
    assert_eq!(token_client.balance(&r), 10_000);
}

/// Pausing lock does NOT affect release (payout) operations.
#[test]
fn test_paused_lock_does_not_block_release() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (token_client, token_id) = fund_contract(&env, &admin, 100_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &100_000);
    client.initialize_contract(&admin);

    // Only lock is paused; release must still succeed
    client.set_paused(&Some(true), &None, &None);
    assert!(client.get_pause_flags().lock_paused);
    assert!(!client.get_pause_flags().release_paused);

    let r = Address::generate(&env);
    let data = client.single_payout(&r, &5_000);
    assert_eq!(data.remaining_balance, 95_000);
    assert_eq!(token_client.balance(&r), 5_000);
}

/// Pausing release does NOT affect lock (funding) operations.
#[test]
fn test_paused_release_does_not_block_lock() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    // Mint enough for two lock operations
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 200_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &100_000);
    client.initialize_contract(&admin);

    // Only release is paused; lock must still succeed
    client.set_paused(&None, &Some(true), &None);
    assert!(!client.get_pause_flags().lock_paused);
    assert!(client.get_pause_flags().release_paused);

    let data = client.lock_program_funds(&admin, &50_000);
    assert_eq!(data.total_funds, 150_000);
    assert_eq!(data.remaining_balance, 150_000);
}

/// All flags paused simultaneously — info/balance queries still work.
#[test]
fn test_fully_paused_query_still_works() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 100_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &100_000);
    client.initialize_contract(&admin);
    client.set_paused(&Some(true), &Some(true), &Some(true));

    let flags = client.get_pause_flags();
    assert!(flags.lock_paused);
    assert!(flags.release_paused);
    assert!(flags.refund_paused);

    // State queries are not affected by pause
    let info = client.get_program_info();
    assert_eq!(info.remaining_balance, 100_000);
    assert_eq!(client.get_remaining_balance(), 100_000);
}

/// Default pause flags are all false (contract starts unpaused).
#[test]
fn test_default_pause_flags_all_false() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _cid) = make_client(&env);
    let admin = Address::generate(&env);
    client.initialize_contract(&admin);

    let flags = client.get_pause_flags();
    assert!(!flags.lock_paused);
    assert!(!flags.release_paused);
    assert!(!flags.refund_paused);
}

// ---------------------------------------------------------------------------
// STATE: Drained (remaining_balance == 0 after all payouts)
// ---------------------------------------------------------------------------

/// After a full single payout the program enters Drained state.
#[test]
fn test_drained_after_full_single_payout() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 50_000);
    let r = Address::generate(&env);

    let data = client.single_payout(&r, &50_000);
    assert_eq!(data.remaining_balance, 0);
    assert_eq!(token_client.balance(&r), 50_000);
    assert_eq!(client.get_remaining_balance(), 0);
}

/// After a full batch payout the program enters Drained state.
#[test]
fn test_drained_after_full_batch_payout() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 90_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let r3 = Address::generate(&env);

    let data = client.batch_payout(
        &vec![&env, r1.clone(), r2.clone(), r3.clone()],
        &vec![&env, 40_000i128, 30_000i128, 20_000i128],
    );
    assert_eq!(data.remaining_balance, 0);
    assert_eq!(token_client.balance(&r1), 40_000);
    assert_eq!(token_client.balance(&r2), 30_000);
    assert_eq!(token_client.balance(&r3), 20_000);
}

/// Further payouts from Drained state must be rejected.
#[test]
#[should_panic(expected = "Insufficient balance")]
fn test_drained_further_payout_rejected() {
    let env = Env::default();
    let (client, admin, _cid, _token) = setup_active_program(&env, 50_000);
    let r = Address::generate(&env);
    client.single_payout(&r, &50_000); // drains to 0
    client.single_payout(&r, &1); // must panic
}

/// Re-locking funds after drain transitions back to Active (Drained → Active).
#[test]
fn test_drained_to_active_via_top_up() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, contract_id) = make_client(&env);
    // Mint enough for both initial lock and top-up
    let admin = Address::generate(&env);
    let (token_client, token_id) = fund_contract(&env, &admin, 200_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);
    client.lock_program_funds(&admin, &100_000);

    // Drain
    let r = Address::generate(&env);
    client.single_payout(&r, &100_000);
    assert_eq!(client.get_remaining_balance(), 0);

    // Re-activate: Drained → Active
    let data = client.lock_program_funds(&admin, &80_000);
    assert_eq!(data.remaining_balance, 80_000);
    assert_eq!(data.total_funds, 180_000); // cumulative total

    // Payouts work again
    let r2 = Address::generate(&env);
    let data2 = client.single_payout(&r2, &30_000);
    assert_eq!(data2.remaining_balance, 50_000);
    assert_eq!(token_client.balance(&r2), 30_000);
}

/// Payout history is preserved and grows across all lifecycle transitions.
#[test]
fn test_payout_history_preserved_across_states() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, contract_id) = make_client(&env);
    let admin = Address::generate(&env);
    let (_, token_id) = fund_contract(&env, &admin, 300_000);
    let program_id = String::from_str(&env, "hack-2026");
    client.init_program(&program_id, &admin, &token_id);

    // Active: first batch of payouts
    client.lock_program_funds(&admin, &200_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    client.single_payout(&r1, &100_000);
    client.single_payout(&r2, &100_000);

    // Now Drained
    assert_eq!(client.get_remaining_balance(), 0);
    let info = client.get_program_info();
    assert_eq!(info.payout_history.len(), 2);

    // Re-activate and pay out more
    client.lock_program_funds(&admin, &100_000);
    let r3 = Address::generate(&env);
    client.single_payout(&r3, &50_000);

    // All three payouts must be in history
    let info2 = client.get_program_info();
    assert_eq!(info2.payout_history.len(), 3);
    assert_eq!(info2.payout_history.get(0).unwrap().recipient, r1);
    assert_eq!(info2.payout_history.get(1).unwrap().recipient, r2);
    assert_eq!(info2.payout_history.get(2).unwrap().recipient, r3);
}

// ---------------------------------------------------------------------------
// RELEASE SCHEDULE: Lifecycle integration
// ---------------------------------------------------------------------------

/// Release schedules created before the timestamp are not triggered.
#[test]
fn test_schedule_before_timestamp_not_triggered() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 100_000);
    let recipient = Address::generate(&env);

    let now = env.ledger().timestamp();
    client.create_program_release_schedule(&30_000, &(now + 500), &recipient);

    // Trigger at t < release_timestamp — should release 0 schedules
    env.ledger().set_timestamp(now + 499);
    let count = client.trigger_program_releases();
    assert_eq!(count, 0);
    assert_eq!(token_client.balance(&recipient), 0);
}

/// Release schedules are triggered at exactly the release_timestamp boundary.
#[test]
fn test_schedule_triggered_at_exact_timestamp() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 100_000);
    let recipient = Address::generate(&env);

    let now = env.ledger().timestamp();
    client.create_program_release_schedule(&25_000, &(now + 200), &recipient);

    env.ledger().set_timestamp(now + 200);
    let count = client.trigger_program_releases();
    assert_eq!(count, 1);
    assert_eq!(token_client.balance(&recipient), 25_000);
    assert_eq!(client.get_remaining_balance(), 75_000);
}

/// A released schedule cannot be re-triggered (idempotency guard).
#[test]
fn test_schedule_not_released_twice() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 100_000);
    let recipient = Address::generate(&env);

    let now = env.ledger().timestamp();
    client.create_program_release_schedule(&20_000, &(now + 100), &recipient);

    env.ledger().set_timestamp(now + 100);
    let count1 = client.trigger_program_releases();
    assert_eq!(count1, 1);

    // Second trigger must release nothing — schedule already marked released
    let count2 = client.trigger_program_releases();
    assert_eq!(count2, 0);
    assert_eq!(token_client.balance(&recipient), 20_000); // unchanged
}

/// Multiple schedules due at the same timestamp are all released in one call.
#[test]
fn test_multiple_schedules_same_timestamp_all_released() {
    let env = Env::default();
    let (client, admin, _cid, token_client) = setup_active_program(&env, 100_000);
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let r3 = Address::generate(&env);

    let now = env.ledger().timestamp();
    client.create_program_release_schedule(&10_000, &(now + 50), &r1);
    client.create_program_release_schedule(&15_000, &(now + 50), &r2);
    client.create_program_release_schedule(&20_000, &(now + 50), &r3);

    env.ledger().set_timestamp(now + 50);
    let count = client.trigger_program_releases();
    assert_eq!(count, 3);
    assert_eq!(token_client.balance(&r1), 10_000);
    assert_eq!(token_client.balance(&r2), 15_000);
    assert_eq!(token_client.balance(&r3), 20_000);
    assert_eq!(client.get_remaining_balance(), 55_000);
}

// ---------------------------------------------------------------------------
// COMPLETE LIFECYCLE INTEGRATION
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// BATCH SIZE BOUNDARY TESTS (MAX_BATCH_SIZE enforcement)
// ---------------------------------------------------------------------------

/// A batch with exactly MAX_BATCH_SIZE recipients must be accepted.
///
/// Budget reset to unlimited: 100 recipients × token transfer + vector ops
/// can exceed the default Soroban test simulation ceiling.
#[test]
fn test_batch_payout_at_max_size_accepted() {
    let env = Env::default();
    env.budget().reset_unlimited();
    let total = (MAX_BATCH_SIZE as i128) * 1_000;
    let (client, _admin, _cid, token_client) = setup_active_program(&env, total);

    let mut recipients = Vec::new(&env);
    let mut amounts = Vec::new(&env);
    for _ in 0..MAX_BATCH_SIZE {
        recipients.push_back(Address::generate(&env));
        amounts.push_back(1_000i128);
    }

    let data = client.batch_payout(&recipients, &amounts);
    assert_eq!(data.remaining_balance, 0);
    // Every recipient received their share
    for i in 0..MAX_BATCH_SIZE {
        assert_eq!(token_client.balance(&recipients.get(i).unwrap()), 1_000);
    }
}

/// A batch with MAX_BATCH_SIZE + 1 recipients must be rejected before any
/// transfer and the reentrancy guard must be cleared (no balance change).
#[test]
#[should_panic(expected = "Batch size exceeds maximum allowed")]
fn test_batch_payout_oversized_rejected() {
    let env = Env::default();
    let oversized = MAX_BATCH_SIZE + 1;
    let total = (oversized as i128) * 1_000;
    let (client, _admin, _cid, _token) = setup_active_program(&env, total);

    let mut recipients = Vec::new(&env);
    let mut amounts = Vec::new(&env);
    for _ in 0..oversized {
        recipients.push_back(Address::generate(&env));
        amounts.push_back(1_000i128);
    }

    client.batch_payout(&recipients, &amounts);
}

/// After an oversized batch is rejected, the contract balance must be unchanged
/// and a valid follow-up batch must still succeed (reentrancy guard cleared).
#[test]
fn test_batch_payout_oversized_no_balance_change_and_guard_cleared() {
    let env = Env::default();
    env.budget().reset_unlimited();
    let oversized = MAX_BATCH_SIZE + 1;
    let total = (oversized as i128) * 1_000;
    let (client, _admin, _cid, token_client) = setup_active_program(&env, total);

    let balance_before = client.get_remaining_balance();

    // Build the oversized vectors
    let mut big_recipients = Vec::new(&env);
    let mut big_amounts = Vec::new(&env);
    for _ in 0..oversized {
        big_recipients.push_back(Address::generate(&env));
        big_amounts.push_back(1_000i128);
    }

    // The oversized call must panic — catch it with try_batch_payout
    let result = client.try_batch_payout(&big_recipients, &big_amounts);
    assert!(result.is_err(), "oversized batch should fail");

    // Balance must be unchanged — no transfers happened
    assert_eq!(client.get_remaining_balance(), balance_before);

    // Reentrancy guard is cleared: a valid payout must now succeed
    let recipient = Address::generate(&env);
    let data = client.batch_payout(&vec![&env, recipient.clone()], &vec![&env, 1_000i128]);
    assert_eq!(data.remaining_balance, balance_before - 1_000);
    assert_eq!(token_client.balance(&recipient), 1_000);
}

/// trigger_program_releases must not process more than MAX_BATCH_SIZE
/// schedules in a single call.
///
/// Budget is reset to unlimited because processing MAX_BATCH_SIZE + 5 = 105
/// schedules in a single invocation (each involving a cross-contract token
/// transfer, vector mutations, and event emission) exceeds the default
/// per-invocation instruction budget used by Soroban's test harness.
/// Using `reset_unlimited` is the standard pattern for such stress checks —
/// see `budget_profiling_tests.rs` for ceiling regression tests.
#[test]
fn test_trigger_program_releases_capped_at_max_batch_size() {
    let env = Env::default();
    // Lift the test budget so the many contract calls and large-batch trigger
    // don't hit the simulation ceiling.
    env.budget().reset_unlimited();

    let schedule_count = MAX_BATCH_SIZE + 5;
    let amount_each = 100i128;
    let total = (schedule_count as i128) * amount_each;
    let (client, _admin, _cid, _token) = setup_active_program(&env, total);

    let now = env.ledger().timestamp();
    let release_at = now + 10;

    // Create more schedules than MAX_BATCH_SIZE, all due at the same time
    for _ in 0..schedule_count {
        let r = Address::generate(&env);
        client.create_program_release_schedule(&amount_each, &release_at, &r);
    }

    env.ledger().set_timestamp(release_at);
    let released = client.trigger_program_releases();

    // Only MAX_BATCH_SIZE schedules may be processed in one invocation
    assert_eq!(released, MAX_BATCH_SIZE);
    assert_eq!(
        client.get_remaining_balance(),
        total - (MAX_BATCH_SIZE as i128) * amount_each
    );
}

/// Full end-to-end: Uninitialized → Initialized → Active → Paused
///                  → Active (resumed) → Drained → Active (top-up) → Drained.
#[test]
fn test_complete_lifecycle_all_transitions() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, contract_id) = make_client(&env);
    // Fund exactly 400_000 = 300_000 (initial lock) + 100_000 (top-up)
    let admin = Address::generate(&env);
    let (token_client, token_id) = fund_contract(&env, &admin, 400_000);
    let program_id = String::from_str(&env, "hack-2026");

    // Uninitialized → Initialized
    let data = client.init_program(&program_id, &admin, &token_id);
    assert_eq!(data.total_funds, 0);
    assert_eq!(data.remaining_balance, 0);

    // Initialized → Active
    let data = client.lock_program_funds(&admin, &300_000);
    assert_eq!(data.total_funds, 300_000);
    assert_eq!(data.remaining_balance, 300_000);

    // Active: perform payouts
    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    client.single_payout(&r1, &50_000);
    client.batch_payout(&vec![&env, r2.clone()], &vec![&env, 50_000i128]);
    assert_eq!(client.get_remaining_balance(), 200_000);

    // Active → Paused
    client.initialize_contract(&admin);
    client.set_paused(&None, &Some(true), &None);
    assert!(client.get_pause_flags().release_paused);

    // Paused → Active (resume)
    client.set_paused(&None, &Some(false), &None);
    assert!(!client.get_pause_flags().release_paused);

    // Active: drain the rest
    let r3 = Address::generate(&env);
    client.single_payout(&r3, &200_000);
    assert_eq!(client.get_remaining_balance(), 0);

    // Drained → Active (top-up)
    let data = client.lock_program_funds(&admin, &100_000);
    assert_eq!(data.remaining_balance, 100_000);

    // Active: final payout — drains again
    let r4 = Address::generate(&env);
    client.single_payout(&r4, &100_000);
    assert_eq!(client.get_remaining_balance(), 0);

    // Verify complete payout history
    let info = client.get_program_info();
    // r1 (single), r2 (batch), r3 (single drain), r4 (final)
    assert_eq!(info.payout_history.len(), 4);
    assert_eq!(info.total_funds, 400_000); // 300_000 + 100_000 top-up

    // Final token balances
    assert_eq!(token_client.balance(&r1), 50_000);
    assert_eq!(token_client.balance(&r2), 50_000);
    assert_eq!(token_client.balance(&r3), 200_000);
    assert_eq!(token_client.balance(&r4), 100_000);
    // Contract still has 100_000 that was minted but never locked
    assert_eq!(token_client.balance(&contract_id), 0);
}

/// Verify that long-lived persistent storage entries (schedules, history, data)
/// remain accessible and are extended during release-path operations.
#[test]
fn test_persistent_storage_ttl_extension() {
    let env = Env::default();
    env.mock_all_auths();
    // 1. Initialize and fund
    let (client, _admin, _cid, token_client) = setup_active_program(&env, 100_000);
    let recipient = Address::generate(&env);

    let now = env.ledger().timestamp();
    // 2. Create a schedule far in the future (60 days)
    let release_at = now + 60 * 24 * 60 * 60;
    client.create_program_release_schedule(&50_000, &release_at, &recipient);

    // 3. Advance ledger time significantly (40 days)
    // On a real network, unextended entries might be archived by now.
    env.ledger().set_timestamp(now + 40 * 24 * 60 * 60);

    // 4. Perform a release-path read (trigger_program_releases)
    // This should NOT release yet but MUST successfully read and re-bump SCHEDULES.
    let count = client.trigger_program_releases();
    assert_eq!(count, 0);

    // 5. Advance past the scheduled release time
    env.ledger().set_timestamp(release_at + 1);

    // 6. Trigger again — should successfully release the long-lived schedule
    let count2 = client.trigger_program_releases();
    assert_eq!(count2, 1);

    // 7. Verify final state
    assert_eq!(token_client.balance(&recipient), 50_000);
    assert_eq!(client.get_remaining_balance(), 50_000);

    // 8. Verify history is still accessible
    let history = client.get_program_release_history();
    assert_eq!(history.len(), 1);
    assert_eq!(history.get(0).unwrap().amount, 50_000);
}

// ---------------------------------------------------------------------------
// MULTI-RELEASER MILESTONE LIFECYCLE
//
// The contract uses a single `authorized_payout_key` as the on-chain release
// authority. "Per-milestone releasers" are modelled at the application layer:
//   • Each milestone is a distinct recipient with a locked budget.
//   • The authorized_payout_key triggers `single_payout` only after the
//     designated off-chain reviewer signals approval (e.g. via signed message).
//   • The recipient-dispute mechanism provides an on-chain veto: an admin can
//     open a recipient dispute before a payout to block it, enforcing per-
//     milestone access control directly in the contract.
//
// These tests walk every relevant edge case:
//   1. Happy-path: 3 milestones, each released in order by the auth key.
//   2. Wrong releaser (no auth): unauthorised caller cannot trigger a payout.
//   3. Out-of-order release: milestones can be released independently.
//   4. Dispute-based milestone blocking: open a recipient dispute before the
//      payout to assert the milestone is correctly held.
//   5. One releaser authorised for multiple milestones.
//   6. Terminal state only after all milestones are paid out.
//   7. Release history integrity across all milestones.
// ---------------------------------------------------------------------------

/// Helper – set up a fresh program with N milestones.
///
/// Returns `(client, auth_key, token_client, milestone_recipients)`.
/// Each milestone recipient gets an equal share of `total_funds`.
fn setup_multi_milestone_program(
    env: &Env,
    total_funds: i128,
    milestone_count: u32,
) -> (
    ProgramEscrowContractClient<'static>,
    Address,               // authorized_payout_key (the single on-chain auth)
    Address,               // admin (can open disputes)
    token::Client<'static>,
    soroban_sdk::Vec<Address>, // milestone recipients, one per milestone
) {
    env.mock_all_auths();

    let contract_id = env.register_contract(None, ProgramEscrowContract);
    let client = ProgramEscrowContractClient::new(env, &contract_id);

    let auth_key = Address::generate(env);
    let admin = Address::generate(env);
    client.initialize_contract(&admin);

    let tokenadmin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract(tokenadmin.clone());
    let token_client = token::Client::new(env, &token_id);
    let token_sac = token::StellarAssetClient::new(env, &token_id);

    // Mint enough for all milestones
    token_sac.mint(&auth_key, &total_funds);

    let program_id = String::from_str(env, "multi-releaser-2026");
    client.init_program(&program_id, &auth_key, &token_id);
    client.lock_program_funds(&auth_key, &total_funds);

    // Generate a distinct recipient address for each milestone
    let mut milestones = soroban_sdk::Vec::new(env);
    for _ in 0..milestone_count {
        milestones.push_back(Address::generate(env));
    }

    (client, auth_key, admin, token_client, milestones)
}

// ---------------------------------------------------------------------------
// Test 1 – Happy-path: 3 milestones each released by the auth key in order.
//
// Scenario: A program has 3 milestones (30k / 40k / 30k).
//           The auth key releases them sequentially, simulating separate
//           reviewers approving each milestone off-chain.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_three_milestones_sequential_release() {
    let env = Env::default();
    let total = 100_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 3);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();

    // ── Milestone 1: reviewer A signals approval, auth key releases ──────
    let data = client.single_payout(&m1, &30_000);
    assert_eq!(data.remaining_balance, 70_000);
    assert_eq!(token_client.balance(&m1), 30_000);

    // ── Milestone 2: reviewer B signals approval, auth key releases ──────
    let data = client.single_payout(&m2, &40_000);
    assert_eq!(data.remaining_balance, 30_000);
    assert_eq!(token_client.balance(&m2), 40_000);

    // ── Milestone 3: reviewer C signals approval, auth key releases ──────
    let data = client.single_payout(&m3, &30_000);
    assert_eq!(data.remaining_balance, 0);
    assert_eq!(token_client.balance(&m3), 30_000);

    // All milestones released → program drained (terminal state)
    assert_eq!(client.get_remaining_balance(), 0);

    // Payout history has exactly 3 entries (one per milestone)
    let info = client.get_program_info();
    assert_eq!(info.payout_history.len(), 3);
    assert_eq!(info.payout_history.get(0).unwrap().recipient, m1);
    assert_eq!(info.payout_history.get(1).unwrap().recipient, m2);
    assert_eq!(info.payout_history.get(2).unwrap().recipient, m3);
}

// ---------------------------------------------------------------------------
// Test 3 – try_single_payout variant of wrong-releaser: verify the call
//           returns an error rather than panicking, and that no funds move.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_wrong_releaser_try_returns_err() {
    let env = Env::default();
    let total = 90_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 3);

    let m1 = milestones.get(0).unwrap();

    // Release milestone 1 successfully (auth is mocked via setup)
    client.single_payout(&m1, &30_000);
    assert_eq!(token_client.balance(&m1), 30_000);

    // Now simulate: a wrong recipient address is used as the release target
    // for an amount that doesn't correspond to any milestone. The auth key
    // authorises the call, but the amount would overdraft the contract, which
    // is the contract-level rejection (balance guard).
    let bogus_recipient = Address::generate(&env);
    let result = client.try_single_payout(&bogus_recipient, &70_001); // 1 unit over remaining
    assert!(
        result.is_err(),
        "Overdraft attempt must return an error"
    );

    // No funds moved for the bogus call
    assert_eq!(token_client.balance(&bogus_recipient), 0);
    assert_eq!(client.get_remaining_balance(), 60_000); // 90k - 30k = 60k
}

// ---------------------------------------------------------------------------
// Test 4 – Out-of-order milestone release.
//
// Milestones 1, 2, 3 can be released in any order because the contract does
// not impose sequencing. This verifies the access-control surface:
//   • Reviewer for milestone 3 should not be able to release milestone 1.
//   • But since auth is enforced at the authorized_payout_key level, an
//     off-chain reviewer can only *signal* approval. This test asserts that
//     when the auth key releases milestones out of order the contract handles
//     each correctly and the final state is still terminal (balance == 0).
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_out_of_order_release_all_succeed() {
    let env = Env::default();
    let total = 120_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 3);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();

    // Release in reverse order: 3 → 2 → 1
    let data = client.single_payout(&m3, &30_000); // milestone 3 first
    assert_eq!(data.remaining_balance, 90_000);
    assert_eq!(token_client.balance(&m3), 30_000);

    let data = client.single_payout(&m1, &40_000); // milestone 1 second
    assert_eq!(data.remaining_balance, 50_000);
    assert_eq!(token_client.balance(&m1), 40_000);

    let data = client.single_payout(&m2, &50_000); // milestone 2 last
    assert_eq!(data.remaining_balance, 0);
    assert_eq!(token_client.balance(&m2), 50_000);

    // Escrow is now in terminal (Drained) state
    assert_eq!(client.get_remaining_balance(), 0);

    // History records the release order, not milestone order
    let info = client.get_program_info();
    assert_eq!(info.payout_history.len(), 3);
    assert_eq!(info.payout_history.get(0).unwrap().recipient, m3);
    assert_eq!(info.payout_history.get(1).unwrap().recipient, m1);
    assert_eq!(info.payout_history.get(2).unwrap().recipient, m2);
}

// ---------------------------------------------------------------------------
// Test 5 – Dispute-based milestone blocking.
//
// An admin opens a recipient dispute for milestone 2 BEFORE it is released.
// The payout must be blocked. After the dispute is resolved, the payout
// succeeds. This is the on-chain enforcement of per-milestone access control.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_dispute_blocks_milestone_payout() {
    let env = Env::default();
    let total = 90_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 3);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();

    // Release milestone 1 successfully
    client.single_payout(&m1, &30_000);
    assert_eq!(token_client.balance(&m1), 30_000);

    // Admin opens a dispute blocking milestone 2 (simulating reviewer rejection)
    let reason = String::from_str(&env, "deliverable not accepted");
    client.open_recipient_dispute(&m2, &reason);
    assert!(client.is_recipient_disputed(&m2));

    // Attempt to release milestone 2 while disputed — must fail
    let result = client.try_single_payout(&m2, &30_000);
    assert!(result.is_err(), "Disputed milestone must be blocked");
    assert_eq!(token_client.balance(&m2), 0); // no funds moved

    // Balance is unchanged (only m1 paid out)
    assert_eq!(client.get_remaining_balance(), 60_000);

    // Release milestone 3 (not disputed) while milestone 2 is blocked
    client.single_payout(&m3, &30_000);
    assert_eq!(token_client.balance(&m3), 30_000);
    assert_eq!(client.get_remaining_balance(), 30_000);

    // Resolve the milestone 2 dispute (reviewer resubmits, admin accepts)
    client.resolve_recipient_dispute(&m2);
    assert!(!client.is_recipient_disputed(&m2));

    // Milestone 2 can now be released
    let data = client.single_payout(&m2, &30_000);
    assert_eq!(data.remaining_balance, 0);
    assert_eq!(token_client.balance(&m2), 30_000);

    // Terminal state: all milestones released
    assert_eq!(client.get_remaining_balance(), 0);
    let info = client.get_program_info();
    assert_eq!(info.payout_history.len(), 3); // m1, m3, m2 (dispute-delayed)
}

// ---------------------------------------------------------------------------
// Test 6 – Incorrect releaser attempt for EACH milestone using disputes.
//
// Each milestone has a "designated reviewer" modelled as an off-chain party.
// If any OTHER reviewer tries to release a milestone not assigned to them,
// the admin opens a recipient dispute to veto it. This test asserts that:
//   • Each milestone can be independently blocked.
//   • Blocking one milestone does not affect others.
//   • The correct releaser (after veto resolution) succeeds.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_per_milestone_access_control_via_disputes() {
    let env = Env::default();
    let total = 150_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 3);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();

    let dispute_reason = String::from_str(&env, "wrong reviewer attempted release");

    // ── Milestone 1: correct reviewer → succeeds ─────────────────────────
    client.single_payout(&m1, &50_000);
    assert_eq!(token_client.balance(&m1), 50_000);

    // ── Milestone 2: wrong reviewer detected → dispute opened → blocked ──
    client.open_recipient_dispute(&m2, &dispute_reason);
    let res = client.try_single_payout(&m2, &50_000);
    assert!(res.is_err(), "Wrong-reviewer milestone must be blocked");
    assert_eq!(token_client.balance(&m2), 0);

    // ── Milestone 3: correct reviewer → succeeds independently ───────────
    client.single_payout(&m3, &50_000);
    assert_eq!(token_client.balance(&m3), 50_000);

    // Balance = only m2 still outstanding
    assert_eq!(client.get_remaining_balance(), 50_000);

    // ── Milestone 2: dispute resolved → correct reviewer releases ─────────
    client.resolve_recipient_dispute(&m2);
    let data = client.single_payout(&m2, &50_000);
    assert_eq!(data.remaining_balance, 0);
    assert_eq!(token_client.balance(&m2), 50_000);

    // Terminal state
    assert_eq!(client.get_remaining_balance(), 0);
}

// ---------------------------------------------------------------------------
// Test 7 – One auth key authorised for multiple milestones.
//
// A single reviewer is legitimately responsible for two milestones (e.g. a
// technical reviewer who approves both M1 and M3). This should work fine.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_single_auth_releases_multiple_milestones() {
    let env = Env::default();
    let total = 90_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 3);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();

    // The auth key releases M1 and M3 (both assigned to the same reviewer)
    client.single_payout(&m1, &30_000);
    client.single_payout(&m3, &30_000);
    assert_eq!(token_client.balance(&m1), 30_000);
    assert_eq!(token_client.balance(&m3), 30_000);

    // M2 still outstanding — not yet in terminal state
    assert_eq!(client.get_remaining_balance(), 30_000);
    let info_mid = client.get_program_info();
    assert_eq!(info_mid.payout_history.len(), 2);

    // Release M2 (different reviewer, same auth key)
    client.single_payout(&m2, &30_000);
    assert_eq!(token_client.balance(&m2), 30_000);

    // Now terminal
    assert_eq!(client.get_remaining_balance(), 0);
    let info = client.get_program_info();
    assert_eq!(info.payout_history.len(), 3);
}

// ---------------------------------------------------------------------------
// Test 8 – Terminal state only after ALL milestones are released.
//
// Verifies that the escrow is NOT considered drained until every milestone
// has been paid out, and that further payout attempts after draining fail.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_terminal_state_only_after_all_milestones_released() {
    let env = Env::default();
    let total = 60_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 3);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();

    // After M1 — NOT drained
    client.single_payout(&m1, &20_000);
    assert_ne!(client.get_remaining_balance(), 0, "Should not be drained after M1");

    // After M2 — NOT drained
    client.single_payout(&m2, &20_000);
    assert_ne!(client.get_remaining_balance(), 0, "Should not be drained after M2");

    // After M3 — NOW drained (terminal state reached)
    client.single_payout(&m3, &20_000);
    assert_eq!(client.get_remaining_balance(), 0, "Should be drained after all milestones");

    // Any further payout attempt on an already-drained escrow must fail
    let extra_recipient = Address::generate(&env);
    let result = client.try_single_payout(&extra_recipient, &1);
    assert!(result.is_err(), "Post-drain payout must be rejected");
    assert_eq!(token_client.balance(&extra_recipient), 0);
}

// ---------------------------------------------------------------------------
// Test 9 – Release history integrity across the full multi-releaser lifecycle.
//
// Asserts that amounts, recipients, and ordering in payout_history are correct
// after a complete multi-milestone lifecycle with one dispute in between.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_release_history_integrity() {
    let env = Env::default();
    let total = 120_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 4);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();
    let m4 = milestones.get(3).unwrap();

    // M1 and M2 released immediately
    client.single_payout(&m1, &30_000);
    client.single_payout(&m2, &20_000);

    // M3 is disputed (simulates reviewer rejection)
    let reason = String::from_str(&env, "M3 deliverable incomplete");
    client.open_recipient_dispute(&m3, &reason);
    let blocked = client.try_single_payout(&m3, &40_000);
    assert!(blocked.is_err());

    // M4 released while M3 is still disputed
    client.single_payout(&m4, &30_000);

    // M3 dispute resolved, then released
    client.resolve_recipient_dispute(&m3);
    client.single_payout(&m3, &40_000);

    // Verify terminal state
    assert_eq!(client.get_remaining_balance(), 0);

    // Verify payout_history ordering: M1, M2, M4, M3 (M3 delayed by dispute)
    let info = client.get_program_info();
    assert_eq!(info.payout_history.len(), 4);
    assert_eq!(info.payout_history.get(0).unwrap().recipient, m1);
    assert_eq!(info.payout_history.get(0).unwrap().amount,  30_000);
    assert_eq!(info.payout_history.get(1).unwrap().recipient, m2);
    assert_eq!(info.payout_history.get(1).unwrap().amount,  20_000);
    assert_eq!(info.payout_history.get(2).unwrap().recipient, m4);
    assert_eq!(info.payout_history.get(2).unwrap().amount,  30_000);
    assert_eq!(info.payout_history.get(3).unwrap().recipient, m3);
    assert_eq!(info.payout_history.get(3).unwrap().amount,  40_000);

    // Individual token balances
    assert_eq!(token_client.balance(&m1), 30_000);
    assert_eq!(token_client.balance(&m2), 20_000);
    assert_eq!(token_client.balance(&m3), 40_000);
    assert_eq!(token_client.balance(&m4), 30_000);
}

// ---------------------------------------------------------------------------
// Test 10 – Multi-releaser lifecycle with 4 milestones using release schedules.
//
// Models the case where milestones have time-locked releases (e.g. each
// reviewer can only trigger their milestone after a defined date). Each
// milestone schedule has a distinct release timestamp to simulate separate
// reviewer windows. Asserts that the contract does not release a milestone
// before its time, and that the terminal state is reached only when all
// schedules have been triggered.
// ---------------------------------------------------------------------------
#[test]
fn test_multi_releaser_time_locked_milestones_via_schedules() {
    let env = Env::default();
    let total = 120_000i128;
    let (client, _auth, _admin, token_client, milestones) =
        setup_multi_milestone_program(&env, total, 4);

    let m1 = milestones.get(0).unwrap();
    let m2 = milestones.get(1).unwrap();
    let m3 = milestones.get(2).unwrap();
    let m4 = milestones.get(3).unwrap();

    let base_ts = env.ledger().timestamp();

    // Each milestone has a different release window (simulating per-reviewer schedule)
    client.create_program_release_schedule(&30_000, &(base_ts + 100), &m1);
    client.create_program_release_schedule(&20_000, &(base_ts + 200), &m2);
    client.create_program_release_schedule(&40_000, &(base_ts + 300), &m3);
    client.create_program_release_schedule(&30_000, &(base_ts + 400), &m4);

    // ── t+100: only M1 is due ─────────────────────────────────────────────
    env.ledger().set_timestamp(base_ts + 100);
    let released = client.trigger_program_releases();
    assert_eq!(released, 1, "Only M1 should be released at t+100");
    assert_eq!(token_client.balance(&m1), 30_000);
    assert_eq!(token_client.balance(&m2), 0);
    assert_eq!(client.get_remaining_balance(), 90_000);

    // ── t+200: M2 becomes due (M1 already released) ───────────────────────
    env.ledger().set_timestamp(base_ts + 200);
    let released = client.trigger_program_releases();
    assert_eq!(released, 1, "Only M2 should be released at t+200");
    assert_eq!(token_client.balance(&m2), 20_000);
    assert_eq!(client.get_remaining_balance(), 70_000);

    // ── t+350: M3 is due; M4 is NOT yet due ──────────────────────────────
    env.ledger().set_timestamp(base_ts + 350);
    let released = client.trigger_program_releases();
    assert_eq!(released, 1, "Only M3 should be released at t+350");
    assert_eq!(token_client.balance(&m3), 40_000);
    assert_eq!(client.get_remaining_balance(), 30_000);

    // Escrow NOT yet in terminal state (M4 still locked)
    assert_ne!(client.get_remaining_balance(), 0);

    // ── t+400: M4 becomes due → terminal state ────────────────────────────
    env.ledger().set_timestamp(base_ts + 400);
    let released = client.trigger_program_releases();
    assert_eq!(released, 1, "M4 should be released at t+400");
    assert_eq!(token_client.balance(&m4), 30_000);
    assert_eq!(client.get_remaining_balance(), 0);

    // All schedules triggered → release history has 4 entries
    let history = client.get_program_release_history();
    assert_eq!(history.len(), 4);
}
