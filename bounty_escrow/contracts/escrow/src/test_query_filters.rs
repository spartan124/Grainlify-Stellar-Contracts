#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env,
};

//  local helpers

fn create_token(
    env: &Env,
    admin: &Address,
) -> (token::Client<'static>, token::StellarAssetClient<'static>) {
    let addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    (
        token::Client::new(env, &addr),
        token::StellarAssetClient::new(env, &addr),
    )
}

fn create_escrow(env: &Env) -> BountyEscrowContractClient<'static> {
    let id = env.register_contract(None, BountyEscrowContract);
    BountyEscrowContractClient::new(env, &id)
}

struct Setup {
    env: Env,
    depositor: Address,
    contributor: Address,
    token: token::Client<'static>,
    token_admin: token::StellarAssetClient<'static>,
    escrow: BountyEscrowContractClient<'static>,
}

impl Setup {
    fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let depositor = Address::generate(&env);
        let contributor = Address::generate(&env);
        let (token, token_admin) = create_token(&env, &admin);
        let escrow = create_escrow(&env);
        escrow.init(&admin, &token.address);
        token_admin.mint(&depositor, &10_000_000);
        Setup {
            env,
            depositor,
            contributor,
            token,
            token_admin,
            escrow,
        }
    }
}

//  status filter tests

#[test]
fn test_query_by_status_locked_returns_only_locked() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);
    s.escrow.release_funds(&2, &s.contributor);

    let results = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Locked, &0, &10);
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        assert_eq!(results.get(i).unwrap().escrow.status, EscrowStatus::Locked);
    }
}

#[test]
fn test_query_by_status_released_returns_only_released() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);
    s.escrow.release_funds(&1, &s.contributor);
    s.escrow.release_funds(&3, &s.contributor);

    let results = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Released, &0, &10);
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        assert_eq!(
            results.get(i).unwrap().escrow.status,
            EscrowStatus::Released
        );
    }
}

#[test]
fn test_query_by_status_refunded_returns_only_refunded() {
    let s = Setup::new();
    let now = s.env.ledger().timestamp();
    let dl = now + 100;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);
    s.escrow.release_funds(&1, &s.contributor);
    s.env.ledger().set_timestamp(dl + 1);
    s.escrow.refund(&2);
    s.escrow.refund(&3);

    let results = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Refunded, &0, &10);
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        assert_eq!(
            results.get(i).unwrap().escrow.status,
            EscrowStatus::Refunded
        );
    }
}

#[test]
fn test_query_by_status_empty_when_no_match() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);

    let results = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Refunded, &0, &10);
    assert_eq!(results.len(), 0);
}

#[test]
fn test_query_by_status_pagination_offset_and_limit() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    for i in 1u64..=5 {
        s.escrow
            .lock_funds(&s.depositor, &i, &(i as i128 * 100), &dl);
    }

    let page1 = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Locked, &0, &2);
    assert_eq!(page1.len(), 2);

    let page2 = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Locked, &2, &2);
    assert_eq!(page2.len(), 2);

    let page3 = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Locked, &4, &2);
    assert_eq!(page3.len(), 1);

    // No overlap between pages
    assert_ne!(
        page1.get(0).unwrap().bounty_id,
        page2.get(0).unwrap().bounty_id
    );
    assert_ne!(
        page2.get(0).unwrap().bounty_id,
        page3.get(0).unwrap().bounty_id
    );
}

// amount filter tests
#[test]
fn test_query_by_amount_range_returns_matching_escrows() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &500, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &1000, &dl);
    s.escrow.lock_funds(&s.depositor, &4, &5000, &dl);

    let results = s.escrow.query_escrows_by_amount(&400, &1100, &0, &10);
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        let amt = results.get(i).unwrap().escrow.amount;
        assert!(amt >= 400 && amt <= 1100);
    }
}

#[test]
fn test_query_by_amount_exact_boundaries_included() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);

    let results = s.escrow.query_escrows_by_amount(&100, &300, &0, &10);
    assert_eq!(results.len(), 3);
}

#[test]
fn test_query_by_amount_no_results_outside_range() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);

    let results = s.escrow.query_escrows_by_amount(&5000, &9999, &0, &10);
    assert_eq!(results.len(), 0);
}

// deadline filter tests

#[test]
fn test_query_by_deadline_range_filters_correctly() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();

    s.escrow.lock_funds(&s.depositor, &1, &100, &(base + 100));
    s.escrow.lock_funds(&s.depositor, &2, &200, &(base + 500));
    s.escrow.lock_funds(&s.depositor, &3, &300, &(base + 1000));
    s.escrow.lock_funds(&s.depositor, &4, &400, &(base + 9999));

    let results = s
        .escrow
        .query_escrows_by_deadline(&(base + 400), &(base + 1500), &0, &10);
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        let dl = results.get(i).unwrap().escrow.deadline;
        assert!(dl >= base + 400 && dl <= base + 1500);
    }
}

#[test]
fn test_query_by_deadline_exact_boundary_included() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();

    s.escrow.lock_funds(&s.depositor, &1, &100, &(base + 200));
    s.escrow.lock_funds(&s.depositor, &2, &200, &(base + 500));
    s.escrow.lock_funds(&s.depositor, &3, &300, &(base + 800));

    let results = s
        .escrow
        .query_escrows_by_deadline(&(base + 200), &(base + 500), &0, &10);
    assert_eq!(results.len(), 2);
}

// depositor filter tests

#[test]
fn test_query_by_depositor_returns_only_that_depositors_escrows() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;
    let depositor2 = Address::generate(&s.env);
    s.token_admin.mint(&depositor2, &10_000);

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&depositor2, &3, &300, &dl);

    let r1 = s.escrow.query_escrows_by_depositor(&s.depositor, &0, &10);
    assert_eq!(r1.len(), 2);
    for i in 0..r1.len() {
        assert_eq!(r1.get(i).unwrap().escrow.depositor, s.depositor);
    }

    let r2 = s.escrow.query_escrows_by_depositor(&depositor2, &0, &10);
    assert_eq!(r2.len(), 1);
    assert_eq!(r2.get(0).unwrap().escrow.depositor, depositor2);
}

#[test]
fn test_query_by_depositor_returns_empty_for_unknown_address() {
    let s = Setup::new();
    let unknown = Address::generate(&s.env);
    let results = s.escrow.query_escrows_by_depositor(&unknown, &0, &10);
    assert_eq!(results.len(), 0);
}

// get_escrow_ids_by_status tests

#[test]
fn test_get_escrow_ids_by_status_returns_correct_ids() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &10, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &20, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &30, &300, &dl);
    s.escrow.release_funds(&20, &s.contributor);

    let locked_ids = s
        .escrow
        .get_escrow_ids_by_status(&EscrowStatus::Locked, &0, &10);
    assert_eq!(locked_ids.len(), 2);
    for i in 0..locked_ids.len() {
        assert_ne!(locked_ids.get(i).unwrap(), 20u64);
    }

    let released_ids = s
        .escrow
        .get_escrow_ids_by_status(&EscrowStatus::Released, &0, &10);
    assert_eq!(released_ids.len(), 1);
    assert_eq!(released_ids.get(0).unwrap(), 20u64);
}

#[test]
fn test_get_escrow_ids_by_status_empty_when_no_match() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;
    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);

    let ids = s
        .escrow
        .get_escrow_ids_by_status(&EscrowStatus::Released, &0, &10);
    assert_eq!(ids.len(), 0);
}

// combined filter test (manual composition)

#[test]
fn test_combined_status_and_amount_filter_via_manual_compose() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &50, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &500, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &5000, &dl);
    s.escrow.release_funds(&2, &s.contributor);

    // Step 1: filter by status=Locked
    let locked = s
        .escrow
        .query_escrows_by_status(&EscrowStatus::Locked, &0, &10);

    // Step 2: among locked, find those with amount >= 1000
    let mut large_count = 0u32;
    let mut large_id = 0u64;
    for i in 0..locked.len() {
        let item = locked.get(i).unwrap();
        if item.escrow.amount >= 1000 {
            large_count += 1;
            large_id = item.bounty_id;
        }
    }
    assert_eq!(large_count, 1);
    assert_eq!(large_id, 3u64);
}

// aggregate stats test

#[test]
fn test_aggregate_stats_reflects_correct_counts_after_lifecycle() {
    let s = Setup::new();
    let now = s.env.ledger().timestamp();
    let dl = now + 100;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);
    s.escrow.lock_funds(&s.depositor, &4, &400, &dl);

    s.escrow.release_funds(&1, &s.contributor);
    s.escrow.release_funds(&2, &s.contributor);

    s.env.ledger().set_timestamp(dl + 1);
    s.escrow.refund(&3);

    let stats = s.escrow.get_aggregate_stats();
    assert_eq!(stats.count_locked, 1);
    assert_eq!(stats.count_released, 2);
    assert_eq!(stats.count_refunded, 1);
    assert_eq!(stats.total_released, 300); // bounties 1+2
    assert_eq!(stats.total_refunded, 300); // bounty 3
    assert_eq!(stats.total_locked, 400); // bounty 4
}

// ==================== COMPOSITE FILTER TESTS ====================
// Testing the new query_escrows function with EscrowQueryFilter

#[test]
fn test_composite_filter_status_and_amount() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    // Create escrows with varying amounts and statuses
    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &500, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &1000, &dl);
    s.escrow.lock_funds(&s.depositor, &4, &5000, &dl);
    s.escrow.release_funds(&2, &s.contributor); // Release one

    // Query: status=Locked AND amount >= 500
    let filter = EscrowQueryFilter {
        has_status_filter: true,
        status: EscrowStatus::Locked,
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 500,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // Should return bounty 3 (1000) and 4 (5000), not 1 (100, too small) or 2 (500 but released)
    assert_eq!(results.len(), 2);
    let mut found_1000 = false;
    let mut found_5000 = false;
    for i in 0..results.len() {
        let item = results.get(i).unwrap();
        assert_eq!(item.escrow.status, EscrowStatus::Locked);
        assert!(item.escrow.amount >= 500);
        if item.escrow.amount == 1000 {
            found_1000 = true;
        }
        if item.escrow.amount == 5000 {
            found_5000 = true;
        }
    }
    assert!(found_1000 && found_5000);
}

#[test]
fn test_composite_filter_status_and_deadline() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();

    // Create escrows with different deadlines
    s.escrow
        .lock_funds(&s.depositor, &1, &100, &(base + 100));
    s.escrow
        .lock_funds(&s.depositor, &2, &200, &(base + 500));
    s.escrow
        .lock_funds(&s.depositor, &3, &300, &(base + 1000));
    s.escrow
        .lock_funds(&s.depositor, &4, &400, &(base + 2000));
    s.escrow.release_funds(&3, &s.contributor); // Release one

    // Query: status=Locked AND deadline <= base+600
    let filter = EscrowQueryFilter {
        has_status_filter: true,
        status: EscrowStatus::Locked,
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: base + 600,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // Should return bounty 1 (base+100) and 2 (base+500), not 3 (released) or 4 (deadline too far)
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        let item = results.get(i).unwrap();
        assert_eq!(item.escrow.status, EscrowStatus::Locked);
        assert!(item.escrow.deadline <= base + 600);
    }
}

#[test]
fn test_composite_filter_depositor_and_status() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;
    let depositor2 = Address::generate(&s.env);
    s.token_admin.mint(&depositor2, &10_000);

    // Create escrows from two depositors
    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&depositor2, &3, &300, &dl);
    s.escrow.lock_funds(&depositor2, &4, &400, &dl);
    s.escrow.release_funds(&1, &s.contributor); // Release depositor1's bounty
    s.escrow.release_funds(&3, &s.contributor); // Release depositor2's bounty

    // Query: depositor=depositor AND status=Locked
    let filter = EscrowQueryFilter {
        has_depositor_filter: true,
        depositor: s.depositor.clone(),
        has_status_filter: true,
        status: EscrowStatus::Locked,
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // Should only return bounty 2 (depositor1, locked)
    assert_eq!(results.len(), 1);
    let item = results.get(0).unwrap();
    assert_eq!(item.bounty_id, 2u64);
    assert_eq!(item.escrow.depositor, s.depositor);
    assert_eq!(item.escrow.status, EscrowStatus::Locked);
}

#[test]
fn test_composite_filter_depositor_status_and_amount() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    // Create multiple escrows from same depositor
    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &500, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &1000, &dl);
    s.escrow.lock_funds(&s.depositor, &4, &5000, &dl);
    s.escrow.release_funds(&3, &s.contributor); // Release one

    // Query: depositor=depositor AND status=Locked AND amount in [400, 2000]
    let filter = EscrowQueryFilter {
        has_depositor_filter: true,
        depositor: s.depositor.clone(),
        has_status_filter: true,
        status: EscrowStatus::Locked,
        min_amount: 400,
        max_amount: 2000,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // Should only return bounty 2 (500, in range and locked)
    assert_eq!(results.len(), 1);
    let item = results.get(0).unwrap();
    assert_eq!(item.bounty_id, 2u64);
    assert_eq!(item.escrow.amount, 500);
}

#[test]
fn test_composite_filter_amount_and_deadline_ranges() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();

    // Create escrows with varying amounts and deadlines
    s.escrow
        .lock_funds(&s.depositor, &1, &100, &(base + 100));
    s.escrow
        .lock_funds(&s.depositor, &2, &500, &(base + 500));
    s.escrow
        .lock_funds(&s.depositor, &3, &1000, &(base + 1000));
    s.escrow
        .lock_funds(&s.depositor, &4, &5000, &(base + 2000));

    // Query: amount in [400, 2000] AND deadline in [base+400, base+1500]
    let filter = EscrowQueryFilter {
        has_status_filter: false,
        status: EscrowStatus::Locked, // Unused
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 400,
        max_amount: 2000,
        min_deadline: base + 400,
        max_deadline: base + 1500,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // Should return bounty 2 (500, base+500) and 3 (1000, base+1000)
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        let item = results.get(i).unwrap();
        assert!(item.escrow.amount >= 400 && item.escrow.amount <= 2000);
        assert!(item.escrow.deadline >= base + 400 && item.escrow.deadline <= base + 1500);
    }
}

#[test]
fn test_composite_filter_all_filters_combined() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();
    let depositor2 = Address::generate(&s.env);
    s.token_admin.mint(&depositor2, &10_000);

    // Create diverse set of escrows
    s.escrow
        .lock_funds(&s.depositor, &1, &100, &(base + 100));
    s.escrow
        .lock_funds(&s.depositor, &2, &500, &(base + 500));
    s.escrow
        .lock_funds(&s.depositor, &3, &1000, &(base + 1000));
    s.escrow
        .lock_funds(&depositor2, &4, &800, &(base + 600));
    s.escrow.release_funds(&3, &s.contributor);

    // Query: depositor=depositor AND status=Locked AND amount in [400, 2000] AND deadline in [base+400, base+1500]
    let filter = EscrowQueryFilter {
        has_depositor_filter: true,
        depositor: s.depositor.clone(),
        has_status_filter: true,
        status: EscrowStatus::Locked,
        min_amount: 400,
        max_amount: 2000,
        min_deadline: base + 400,
        max_deadline: base + 1500,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // Should only return bounty 2 (500, base+500)
    assert_eq!(results.len(), 1);
    let item = results.get(0).unwrap();
    assert_eq!(item.bounty_id, 2u64);
    assert_eq!(item.escrow.depositor, s.depositor);
    assert_eq!(item.escrow.status, EscrowStatus::Locked);
    assert_eq!(item.escrow.amount, 500);
    assert_eq!(item.escrow.deadline, base + 500);
}

#[test]
fn test_composite_filter_empty_filter_returns_all() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);

    // Empty filter (all disabled) should return all escrows
    let filter = EscrowQueryFilter {
        has_status_filter: false,
        status: EscrowStatus::Locked, // Unused
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    assert_eq!(results.len(), 3);
}

#[test]
fn test_composite_filter_no_matches() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();

    s.escrow.lock_funds(&s.depositor, &1, &100, &(base + 100));
    s.escrow.lock_funds(&s.depositor, &2, &200, &(base + 200));

    // Query with impossible conditions
    let filter = EscrowQueryFilter {
        has_status_filter: true,
        status: EscrowStatus::Locked,
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 1000, // No escrow has this amount
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    assert_eq!(results.len(), 0);
}

#[test]
fn test_composite_filter_pagination_offset_and_limit() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    // Create 5 locked escrows
    for i in 1u64..=5 {
        s.escrow
            .lock_funds(&s.depositor, &i, &(i as i128 * 100), &dl);
    }

    let filter = EscrowQueryFilter {
        has_status_filter: true,
        status: EscrowStatus::Locked,
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };

    // Page 1: offset=0, limit=2
    let page1 = s.escrow.query_escrows(&filter, &0, &2);
    assert_eq!(page1.len(), 2);

    // Page 2: offset=2, limit=2
    let page2 = s.escrow.query_escrows(&filter, &2, &2);
    assert_eq!(page2.len(), 2);

    // Page 3: offset=4, limit=2 (only 1 remaining)
    let page3 = s.escrow.query_escrows(&filter, &4, &2);
    assert_eq!(page3.len(), 1);

    // Verify no overlap between pages
    assert_ne!(
        page1.get(0).unwrap().bounty_id,
        page2.get(0).unwrap().bounty_id
    );
    assert_ne!(
        page2.get(0).unwrap().bounty_id,
        page3.get(0).unwrap().bounty_id
    );
}

#[test]
fn test_composite_filter_pagination_with_filters() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    // Create 10 escrows, only half match filter
    for i in 1u64..=10 {
        s.escrow
            .lock_funds(&s.depositor, &i, &(i as i128 * 100), &dl);
    }

    // Release even-numbered bounties (2, 4, 6, 8, 10)
    for i in (2u64..=10).step_by(2) {
        s.escrow.release_funds(&i, &s.contributor);
    }

    // Query locked escrows (should be 1, 3, 5, 7, 9)
    let filter = EscrowQueryFilter {
        has_status_filter: true,
        status: EscrowStatus::Locked,
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };

    let page1 = s.escrow.query_escrows(&filter, &0, &2);
    assert_eq!(page1.len(), 2);

    let page2 = s.escrow.query_escrows(&filter, &2, &2);
    assert_eq!(page2.len(), 2);

    let page3 = s.escrow.query_escrows(&filter, &4, &2);
    assert_eq!(page3.len(), 1);

    // All results should be locked
    for i in 0..page1.len() {
        assert_eq!(page1.get(i).unwrap().escrow.status, EscrowStatus::Locked);
    }
    for i in 0..page2.len() {
        assert_eq!(page2.get(i).unwrap().escrow.status, EscrowStatus::Locked);
    }
    for i in 0..page3.len() {
        assert_eq!(page3.get(i).unwrap().escrow.status, EscrowStatus::Locked);
    }
}

#[test]
fn test_composite_filter_boundary_values_amount() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);

    // Test exact boundaries are inclusive
    let filter = EscrowQueryFilter {
        has_status_filter: false,
        status: EscrowStatus::Locked, // Unused
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 100,
        max_amount: 300,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // All 3 should match (boundaries are inclusive)
    assert_eq!(results.len(), 3);
}

#[test]
fn test_composite_filter_boundary_values_deadline() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();

    s.escrow.lock_funds(&s.depositor, &1, &100, &(base + 100));
    s.escrow.lock_funds(&s.depositor, &2, &200, &(base + 200));
    s.escrow.lock_funds(&s.depositor, &3, &300, &(base + 300));

    // Test exact boundaries are inclusive
    let filter = EscrowQueryFilter {
        has_status_filter: false,
        status: EscrowStatus::Locked, // Unused
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: base + 100,
        max_deadline: base + 300,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // All 3 should match (boundaries are inclusive)
    assert_eq!(results.len(), 3);
}

#[test]
fn test_composite_filter_depositor_index_optimization() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;
    let depositor2 = Address::generate(&s.env);
    let depositor3 = Address::generate(&s.env);
    s.token_admin.mint(&depositor2, &10_000);
    s.token_admin.mint(&depositor3, &10_000);

    // Create many escrows from different depositors
    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&depositor2, &2, &200, &dl);
    s.escrow.lock_funds(&depositor3, &3, &300, &dl);
    s.escrow.lock_funds(&s.depositor, &4, &400, &dl);
    s.escrow.lock_funds(&depositor2, &5, &500, &dl);
    s.escrow.lock_funds(&depositor3, &6, &600, &dl);

    // Query with depositor filter - should use depositor index
    let filter = EscrowQueryFilter {
        has_depositor_filter: true,
        depositor: s.depositor.clone(),
        has_status_filter: false,
        status: EscrowStatus::Locked, // Unused
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    // Should return only 2 escrows from depositor
    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        assert_eq!(results.get(i).unwrap().escrow.depositor, s.depositor);
    }
}

#[test]
fn test_composite_filter_refunded_status() {
    let s = Setup::new();
    let now = s.env.ledger().timestamp();
    let dl = now + 100;

    s.escrow.lock_funds(&s.depositor, &1, &100, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &200, &dl);
    s.escrow.lock_funds(&s.depositor, &3, &300, &dl);

    s.escrow.release_funds(&1, &s.contributor);

    s.env.ledger().set_timestamp(dl + 1);
    s.escrow.refund(&2);
    s.escrow.refund(&3);

    // Query refunded escrows
    let filter = EscrowQueryFilter {
        has_status_filter: true,
        status: EscrowStatus::Refunded,
        has_depositor_filter: false,
        depositor: Address::generate(&s.env), // Unused
        min_amount: 0,
        max_amount: i128::MAX,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    let results = s.escrow.query_escrows(&filter, &0, &10);

    assert_eq!(results.len(), 2);
    for i in 0..results.len() {
        assert_eq!(
            results.get(i).unwrap().escrow.status,
            EscrowStatus::Refunded
        );
    }
}

#[test]
fn test_query_filters_combined_three_dimensions_exact_subset() {
    let s = Setup::new();
    let base = s.env.ledger().timestamp();
    let dl1 = base + 1000;
    
    let depositor2 = Address::generate(&s.env);
    s.token_admin.mint(&depositor2, &10_000);

    // 1. match (depositor, locked, amount=500)
    s.escrow.lock_funds(&s.depositor, &1, &500, &dl1); 
    // 2. wrong depositor
    s.escrow.lock_funds(&depositor2, &2, &500, &dl1);
    // 3. wrong amount
    s.escrow.lock_funds(&s.depositor, &3, &200, &dl1);
    // 4. wrong status (released)
    s.escrow.lock_funds(&s.depositor, &4, &500, &dl1);
    s.escrow.release_funds(&4, &s.contributor);
    // 5. match (depositor, locked, amount=600)
    s.escrow.lock_funds(&s.depositor, &5, &600, &dl1);

    // Query: depositor=s.depositor, status=Locked, amount in [400, 700]
    let filter = EscrowQueryFilter {
        has_depositor_filter: true,
        depositor: s.depositor.clone(),
        has_status_filter: true,
        status: EscrowStatus::Locked,
        min_amount: 400,
        max_amount: 700,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    
    let results = s.escrow.query_escrows(&filter, &0, &10);
    assert_eq!(results.len(), 2);
    let mut found_1 = false;
    let mut found_5 = false;
    for i in 0..results.len() {
        let id = results.get(i).unwrap().bounty_id;
        if id == 1 { found_1 = true; }
        if id == 5 { found_5 = true; }
    }
    assert!(found_1 && found_5);
}

#[test]
fn test_query_filters_combined_matching_zero() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;
    
    // Create some escrows
    s.escrow.lock_funds(&s.depositor, &1, &500, &dl);
    s.escrow.lock_funds(&s.depositor, &2, &600, &dl);
    
    // Query with 3 dimensions that don't overlap with any escrow:
    // status = Released, depositor = s.depositor, amount in [1000, 2000]
    let filter = EscrowQueryFilter {
        has_depositor_filter: true,
        depositor: s.depositor.clone(),
        has_status_filter: true,
        status: EscrowStatus::Released,
        min_amount: 1000,
        max_amount: 2000,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    
    let results = s.escrow.query_escrows(&filter, &0, &10);
    // Should cleanly return empty
    assert_eq!(results.len(), 0);
}

#[test]
fn test_query_filters_mutually_exclusive() {
    let s = Setup::new();
    let dl = s.env.ledger().timestamp() + 1000;
    
    s.escrow.lock_funds(&s.depositor, &1, &500, &dl);
    
    // Mutually exclusive: min_amount > max_amount
    let filter = EscrowQueryFilter {
        has_depositor_filter: false,
        depositor: Address::generate(&s.env),
        has_status_filter: false,
        status: EscrowStatus::Locked, // Unused
        min_amount: 1000,
        max_amount: 500,
        min_deadline: 0,
        max_deadline: u64::MAX,
    };
    
    let results = s.escrow.query_escrows(&filter, &0, &10);
    // Should cleanly return empty, as it's logically impossible
    assert_eq!(results.len(), 0);
}
