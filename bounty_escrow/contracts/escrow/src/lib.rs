#![no_std]
mod events;
mod governance_integration;
mod analytics;
mod error_recovery;

#[cfg(test)]
mod test_rbac;
mod test_admin_authz;
mod test_coverage_boost;
mod test_coverage_boost_small;
mod test_coverage_comprehensive;
mod test_serialization;

use events::{
    emit_batch_funds_locked, emit_batch_funds_released, emit_bounty_expired,
    emit_bounty_initialized, emit_funds_locked, emit_funds_refunded, emit_funds_released,
    emit_claim_created, emit_claim_executed, emit_claim_cancelled,
    BatchFundsLocked, BatchFundsReleased, BountyEscrowInitialized, BountyExpired,
    ClaimCancelled, ClaimCreated, ClaimExecuted,
    FundsLocked, FundsRefunded, FundsReleased, EVENT_VERSION_V2,
};
use analytics::{
    emit_analytics_snapshot, emit_bounty_activity, emit_bounty_state_transitioned,
    get_bounty_analytics, init_bounty_analytics, update_analytics_on_refund,
    update_analytics_on_release, BountyActivityEvent, BountyStateTransitioned, ContractAnalytics,
    AnalyticsSnapshot,
};
use error_recovery::{
    check_and_allow, record_failure, record_success, set_circuit_admin, get_circuit_admin,
    set_config, get_config, get_status, reset_circuit_breaker, CircuitBreakerConfig,
    CircuitBreakerStatus, CircuitState, ErrorEntry, ERR_CIRCUIT_OPEN,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, vec, Address, Env,
    Symbol, Vec,
};

// ==================== MONITORING MODULE ====================
mod monitoring {
    use soroban_sdk::{contracttype, symbol_short, Address, Env, String, Symbol};

    // Storage keys
    const OPERATION_COUNT: &str = "op_count";
    const USER_COUNT: &str = "usr_count";
    const ERROR_COUNT: &str = "err_count";

    // Event: Operation metric
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct OperationMetric {
        pub operation: Symbol,
        pub caller: Address,
        pub timestamp: u64,
        pub success: bool,
    }

    // Event: Performance metric
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct PerformanceMetric {
        pub function: Symbol,
        pub duration: u64,
        pub timestamp: u64,
    }

    // Data: Health status
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct HealthStatus {
        pub is_healthy: bool,
        pub last_operation: u64,
        pub total_operations: u64,
        pub contract_version: String,
    }

    // Data: Analytics
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct Analytics {
        pub operation_count: u64,
        pub unique_users: u64,
        pub error_count: u64,
        pub error_rate: u32,
    }

    // Data: State snapshot
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct StateSnapshot {
        pub timestamp: u64,
        pub total_operations: u64,
        pub total_users: u64,
        pub total_errors: u64,
    }

    // Data: Performance stats
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct PerformanceStats {
        pub function_name: Symbol,
        pub call_count: u64,
        pub total_time: u64,
        pub avg_time: u64,
        pub last_called: u64,
    }

    // Track operation
    pub fn track_operation(env: &Env, operation: Symbol, caller: Address, success: bool) {
        let key = Symbol::new(env, OPERATION_COUNT);
        let count: u64 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(count + 1));

        if !success {
            let err_key = Symbol::new(env, ERROR_COUNT);
            let err_count: u64 = env.storage().persistent().get(&err_key).unwrap_or(0);
            env.storage().persistent().set(&err_key, &(err_count + 1));
        }

        env.events().publish(
            (symbol_short!("metric"), symbol_short!("op")),
            OperationMetric {
                operation,
                caller,
                timestamp: env.ledger().timestamp(),
                success,
            },
        );
    }

    // Track performance
    pub fn emit_performance(env: &Env, function: Symbol, duration: u64) {
        let count_key = (Symbol::new(env, "perf_cnt"), function.clone());
        let time_key = (Symbol::new(env, "perf_time"), function.clone());

        let count: u64 = env.storage().persistent().get(&count_key).unwrap_or(0);
        let total: u64 = env.storage().persistent().get(&time_key).unwrap_or(0);

        env.storage().persistent().set(&count_key, &(count + 1));
        env.storage()
            .persistent()
            .set(&time_key, &(total + duration));

        env.events().publish(
            (symbol_short!("metric"), symbol_short!("perf")),
            PerformanceMetric {
                function,
                duration,
                timestamp: env.ledger().timestamp(),
            },
        );
    }

    // Health check
    pub fn health_check(env: &Env) -> HealthStatus {
        let key = Symbol::new(env, OPERATION_COUNT);
        let ops: u64 = env.storage().persistent().get(&key).unwrap_or(0);

        HealthStatus {
            is_healthy: true,
            last_operation: env.ledger().timestamp(),
            total_operations: ops,
            contract_version: String::from_str(env, "1.0.0"),
        }
    }

    // Get analytics
    pub fn get_analytics(env: &Env) -> Analytics {
        let op_key = Symbol::new(env, OPERATION_COUNT);
        let usr_key = Symbol::new(env, USER_COUNT);
        let err_key = Symbol::new(env, ERROR_COUNT);

        let ops: u64 = env.storage().persistent().get(&op_key).unwrap_or(0);
        let users: u64 = env.storage().persistent().get(&usr_key).unwrap_or(0);
        let errors: u64 = env.storage().persistent().get(&err_key).unwrap_or(0);

        let error_rate = if ops > 0 {
            ((errors as u128 * 10000) / ops as u128) as u32
        } else {
            0
        };

        Analytics {
            operation_count: ops,
            unique_users: users,
            error_count: errors,
            error_rate,
        }
    }

    // Get state snapshot
    pub fn get_state_snapshot(env: &Env) -> StateSnapshot {
        let op_key = Symbol::new(env, OPERATION_COUNT);
        let usr_key = Symbol::new(env, USER_COUNT);
        let err_key = Symbol::new(env, ERROR_COUNT);

        StateSnapshot {
            timestamp: env.ledger().timestamp(),
            total_operations: env.storage().persistent().get(&op_key).unwrap_or(0),
            total_users: env.storage().persistent().get(&usr_key).unwrap_or(0),
            total_errors: env.storage().persistent().get(&err_key).unwrap_or(0),
        }
    }

    // Get performance stats
    pub fn get_performance_stats(env: &Env, function_name: Symbol) -> PerformanceStats {
        let count_key = (Symbol::new(env, "perf_cnt"), function_name.clone());
        let time_key = (Symbol::new(env, "perf_time"), function_name.clone());
        let last_key = (Symbol::new(env, "perf_last"), function_name.clone());

        let count: u64 = env.storage().persistent().get(&count_key).unwrap_or(0);
        let total: u64 = env.storage().persistent().get(&time_key).unwrap_or(0);
        let last: u64 = env.storage().persistent().get(&last_key).unwrap_or(0);

        let avg = if count > 0 { total / count } else { 0 };

        PerformanceStats {
            function_name,
            call_count: count,
            total_time: total,
            avg_time: avg,
            last_called: last,
        }
    }
}
// ==================== END MONITORING MODULE ====================

// ==================== ANTI-ABUSE MODULE ====================
mod anti_abuse {
    use soroban_sdk::{contracttype, symbol_short, Address, Env};

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AntiAbuseConfig {
        pub window_size: u64,     // Window size in seconds
        pub max_operations: u32,  // Max operations allowed in window
        pub cooldown_period: u64, // Minimum seconds between operations
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct AddressState {
        pub last_operation_timestamp: u64,
        pub window_start_timestamp: u64,
        pub operation_count: u32,
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum AntiAbuseKey {
        Config,
        State(Address),
        Whitelist(Address),
        Admin,
    }

    pub fn get_config(env: &Env) -> AntiAbuseConfig {
        env.storage()
            .instance()
            .get(&AntiAbuseKey::Config)
            .unwrap_or(AntiAbuseConfig {
                window_size: 3600, // 1 hour default
                max_operations: 100,
                cooldown_period: 60, // 1 minute default
            })
    }

    pub fn set_config(env: &Env, config: AntiAbuseConfig) {
        env.storage().instance().set(&AntiAbuseKey::Config, &config);
    }

    pub fn is_whitelisted(env: &Env, address: Address) -> bool {
        env.storage()
            .instance()
            .has(&AntiAbuseKey::Whitelist(address))
    }

    pub fn set_whitelist(env: &Env, address: Address, whitelisted: bool) {
        if whitelisted {
            env.storage()
                .instance()
                .set(&AntiAbuseKey::Whitelist(address), &true);
        } else {
            env.storage()
                .instance()
                .remove(&AntiAbuseKey::Whitelist(address));
        }
    }

    pub fn get_admin(env: &Env) -> Option<Address> {
        env.storage().instance().get(&AntiAbuseKey::Admin)
    }

    pub fn set_admin(env: &Env, admin: Address) {
        env.storage().instance().set(&AntiAbuseKey::Admin, &admin);
    }

    pub fn check_rate_limit(env: &Env, address: Address) {
        if is_whitelisted(env, address.clone()) {
            return;
        }

        let config = get_config(env);
        let now = env.ledger().timestamp();
        let key = AntiAbuseKey::State(address.clone());

        let mut state: AddressState =
            env.storage()
                .persistent()
                .get(&key)
                .unwrap_or(AddressState {
                    last_operation_timestamp: 0,
                    window_start_timestamp: now,
                    operation_count: 0,
                });

        // 1. Cooldown check
        if state.last_operation_timestamp > 0
            && now
                < state
                    .last_operation_timestamp
                    .saturating_add(config.cooldown_period)
        {
            env.events().publish(
                (symbol_short!("abuse"), symbol_short!("cooldown")),
                (address.clone(), now),
            );
            panic!("Operation in cooldown period");
        }

        // 2. Window check
        if now
            >= state
                .window_start_timestamp
                .saturating_add(config.window_size)
        {
            // New window
            state.window_start_timestamp = now;
            state.operation_count = 1;
        } else {
            // Same window
            if state.operation_count >= config.max_operations {
                env.events().publish(
                    (symbol_short!("abuse"), symbol_short!("limit")),
                    (address.clone(), now),
                );
                panic!("Rate limit exceeded");
            }
            state.operation_count += 1;
        }

        state.last_operation_timestamp = now;
        env.storage().persistent().set(&key, &state);

        // Extend TTL for state (approx 1 day)
        env.storage().persistent().extend_ttl(&key, 17280, 17280);
    }
}
// ==================== END ANTI-ABUSE MODULE ====================

// ==================== CONSTANTS ====================
const BASIS_POINTS: i128 = 10_000;
const MAX_FEE_RATE: i128 = 5_000; // 50% max fee
const MAX_BATCH_SIZE: u32 = 20;
/// Extend escrow persistent entries when the ledger sequence is within roughly one day of expiry.
const ESCROW_TTL_THRESHOLD: u32 = 17_280;
/// Keep escrow persistent entries alive for roughly thirty days on five-second ledgers.
const ESCROW_TTL_EXTEND_TO: u32 = 518_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    BountyExists = 3,
    BountyNotFound = 4,
    FundsNotLocked = 5,
    DeadlineNotPassed = 6,
    Unauthorized = 7,
    InvalidFeeRate = 8,
    FeeRecipientNotSet = 9,
    InvalidBatchSize = 10,
    BatchSizeMismatch = 11,
    DuplicateBountyId = 12,
    /// Returned when amount is invalid (zero, negative, or exceeds available)
    InvalidAmount = 13,
    /// Returned when deadline is invalid (in the past or too far in the future)
    InvalidDeadline = 14,
    /// Returned when contract has insufficient funds for the operation
    InsufficientFunds = 16,
    /// Returned when refund is attempted without admin approval
    RefundNotApproved = 17,
    FundsPaused = 18,
    /// Returned when lock amount is below the configured policy minimum (Issue #62)
    AmountBelowMinimum = 19,
    /// Returned when lock amount is above the configured policy maximum (Issue #62)
    AmountAboveMaximum = 20,
    /// Returned when circuit breaker is open and operations are paused
    CircuitBreakerOpen = 21,
    /// Returned when an authorized claim is attempted after its claim window expires
    ClaimExpired = 22,
    /// Returned when the linked governance contract version is below the configured minimum
    GovernanceVersionTooLow = 23,
    /// Returned when authorize_claim is called while a non-expired pending claim already exists
    PendingClaimExists = 24,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EscrowStatus {
    Locked,
    Released,
    Refunded,
    PartiallyRefunded,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Escrow {
    pub depositor: Address,
    /// Total amount originally locked into this escrow.
    pub amount: i128,
    /// Amount still available for release; decremented on each partial_release.
    /// Reaches 0 when fully paid out, at which point status becomes Released.
    pub remaining_amount: i128,
    pub status: EscrowStatus,
    pub deadline: u64,
    pub refund_history: Vec<RefundRecord>,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    Escrow(u64),             // bounty_id
    EscrowIndex,             // Vec<u64> of all bounty_ids
    DepositorIndex(Address), // Vec<u64> of bounty_ids by depositor
    FeeConfig,               // Fee configuration
    RefundApproval(u64),     // bounty_id -> RefundApproval
    ReentrancyGuard,
    MultisigConfig,
    ReleaseApproval(u64), // bounty_id -> ReleaseApproval
    PendingClaim(u64),    // bounty_id -> ClaimRecord
    ClaimWindow,          // u64 seconds (global config)
    PauseFlags,           // PauseFlags struct
    AmountPolicy, // Option<(i128, i128)> — (min_amount, max_amount) set by set_amount_policy
    AggregateCounters, // AggregateStats — O(1) incremental counters maintained on state transitions
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowWithId {
    pub bounty_id: u64,
    pub escrow: Escrow,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PauseFlags {
    pub global_paused: bool,
    pub lock_paused: bool,
    pub release_paused: bool,
    pub refund_paused: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregateStats {
    pub total_locked: i128,
    pub total_released: i128,
    pub total_refunded: i128,
    pub count_locked: u32,
    pub count_released: u32,
    pub count_refunded: u32,
}

/// Composite filter for querying escrows.
/// Multiple filters can be combined for rich querying capabilities.
///
/// To indicate "no filter" for a field, use these sentinel values:
/// - has_status_filter: false means ignore status
/// - has_depositor_filter: false means ignore depositor
/// - min_amount: 0 means no minimum
/// - max_amount: i128::MAX means no maximum
/// - min_deadline: 0 means no minimum
/// - max_deadline: u64::MAX means no maximum
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowQueryFilter {
    pub has_status_filter: bool,
    pub status: EscrowStatus,
    pub has_depositor_filter: bool,
    pub depositor: Address,
    pub min_amount: i128,
    pub max_amount: i128,
    pub min_deadline: u64,
    pub max_deadline: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PauseStateChanged {
    pub operation: Symbol,
    pub paused: bool,
    pub admin: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeConfig {
    pub lock_fee_rate: i128,
    pub release_fee_rate: i128,
    pub fee_recipient: Address,
    pub fee_enabled: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultisigConfig {
    pub threshold_amount: i128,
    pub signers: Vec<Address>,
    pub required_signatures: u32,
}

/// Compact, admin-focused configuration snapshot for audit views.
///
/// This struct is intentionally stable and versioned so that off-chain
/// dashboards can safely decode it across contract upgrades.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminConfigSnapshot {
    /// Schema version for this snapshot.
    pub version: u32,
    /// Contract admin address responsible for configuration changes.
    pub admin: Address,
    /// Escrow token address used for all bounties.
    pub token: Address,
    /// Current fee configuration (rates, recipient, enablement).
    pub fee_config: FeeConfig,
    /// Granular pause flags controlling lock/release/refund operations.
    pub pause_flags: PauseFlags,
    /// Optional governance contract controlling upgrades and config.
    pub governance_contract: Option<Address>,
    /// Minimum governance version required for admin operations.
    pub min_governance_version: u32,
    /// Global claim window in seconds for authorized claims.
    pub claim_window: u64,
    /// Whether an amount policy is configured.
    pub has_amount_policy: bool,
    /// Minimum allowed lock amount when `has_amount_policy` is true.
    pub min_lock_amount: i128,
    /// Maximum allowed lock amount when `has_amount_policy` is true.
    pub max_lock_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseApproval {
    pub bounty_id: u64,
    pub contributor: Address,
    pub approvals: Vec<Address>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimRecord {
    pub bounty_id: u64,
    pub recipient: Address,
    pub amount: i128,
    pub expires_at: u64,
    pub claimed: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefundMode {
    Full,
    Partial,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundApproval {
    pub bounty_id: u64,
    pub amount: i128,
    pub recipient: Address,
    pub mode: RefundMode,
    pub approved_by: Address,
    pub approved_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundRecord {
    pub amount: i128,
    pub recipient: Address,
    pub timestamp: u64,
    pub mode: RefundMode,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockFundsItem {
    pub bounty_id: u64,
    pub depositor: Address,
    pub amount: i128,
    pub deadline: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseFundsItem {
    pub bounty_id: u64,
    pub contributor: Address,
}

#[contract]
pub struct BountyEscrowContract;

#[contractimpl]
impl BountyEscrowContract {
    /// Bump the TTL for the primary escrow entry after escrow reads and writes.
    fn bump_escrow_ttl(env: &Env, bounty_id: u64) {
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(bounty_id),
            ESCROW_TTL_THRESHOLD,
            ESCROW_TTL_EXTEND_TO,
        );
    }

    /// Bump the TTL for escrow indexes that point callers back to a bounty id.
    fn bump_escrow_index_ttl(env: &Env, depositor: Address) {
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowIndex,
            ESCROW_TTL_THRESHOLD,
            ESCROW_TTL_EXTEND_TO,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::DepositorIndex(depositor),
            ESCROW_TTL_THRESHOLD,
            ESCROW_TTL_EXTEND_TO,
        );
    }

    // ==================== INCREMENTAL AGGREGATE COUNTERS ====================

    /// Get the current aggregate counters, initializing to zero if not present.
    ///
    /// These counters are maintained incrementally on every state transition
    /// (lock, release, refund, partial_release) to provide O(1) aggregate queries.
    fn get_counters(env: &Env) -> AggregateStats {
        env.storage()
            .persistent()
            .get(&DataKey::AggregateCounters)
            .unwrap_or(AggregateStats {
                total_locked: 0,
                total_released: 0,
                total_refunded: 0,
                count_locked: 0,
                count_released: 0,
                count_refunded: 0,
            })
    }

    /// Save the aggregate counters back to storage.
    fn set_counters(env: &Env, stats: &AggregateStats) {
        env.storage()
            .persistent()
            .set(&DataKey::AggregateCounters, stats);
        env.storage().persistent().extend_ttl(
            &DataKey::AggregateCounters,
            ESCROW_TTL_THRESHOLD,
            ESCROW_TTL_EXTEND_TO,
        );
    }

    /// Increment counters when a new bounty is locked.
    ///
    /// This is called from `lock_funds` and `batch_lock_funds`.
    /// Adds the amount to total_locked and increments count_locked.
    fn increment_locked(env: &Env, amount: i128) {
        let mut stats = Self::get_counters(env);
        stats.total_locked = stats.total_locked.saturating_add(amount);
        stats.count_locked = stats.count_locked.saturating_add(1);
        Self::set_counters(env, &stats);
    }

    /// Transition a bounty from Locked to Released.
    ///
    /// Called from `release_funds`, `batch_release_funds`, and `claim`.
    /// Decrements locked counters and increments released counters.
    fn transition_locked_to_released(env: &Env, amount: i128) {
        let mut stats = Self::get_counters(env);
        stats.total_locked = stats.total_locked.saturating_sub(amount);
        stats.count_locked = stats.count_locked.saturating_sub(1);
        stats.total_released = stats.total_released.saturating_add(amount);
        stats.count_released = stats.count_released.saturating_add(1);
        Self::set_counters(env, &stats);
    }

    /// Transition a bounty from Locked to Refunded.
    ///
    /// Called from `refund` and `sweep_expired_refunds` when the full amount is refunded.
    /// Decrements locked counters and increments refunded counters.
    fn transition_locked_to_refunded(env: &Env, amount: i128) {
        let mut stats = Self::get_counters(env);
        stats.total_locked = stats.total_locked.saturating_sub(amount);
        stats.count_locked = stats.count_locked.saturating_sub(1);
        stats.total_refunded = stats.total_refunded.saturating_add(amount);
        stats.count_refunded = stats.count_refunded.saturating_add(1);
        Self::set_counters(env, &stats);
    }

    /// Transition a bounty from Locked to PartiallyRefunded.
    ///
    /// Called from `refund` when a partial refund is performed.
    /// The bounty remains in the locked bucket but total_locked is reduced by the refunded amount.
    /// Total_refunded tracks the cumulative refunded amount.
    fn partial_refund_from_locked(env: &Env, refund_amount: i128) {
        let mut stats = Self::get_counters(env);
        stats.total_locked = stats.total_locked.saturating_sub(refund_amount);
        stats.total_refunded = stats.total_refunded.saturating_add(refund_amount);
        // Note: count_locked stays the same as the bounty is still active (PartiallyRefunded)
        Self::set_counters(env, &stats);
    }

    /// Transition a bounty from PartiallyRefunded to Refunded.
    ///
    /// Called from `refund` when the final refund completes a partially refunded bounty.
    /// Decrements locked count and increments refunded count.
    fn transition_partially_refunded_to_refunded(env: &Env, final_refund_amount: i128) {
        let mut stats = Self::get_counters(env);
        stats.total_locked = stats.total_locked.saturating_sub(final_refund_amount);
        stats.count_locked = stats.count_locked.saturating_sub(1);
        stats.total_refunded = stats.total_refunded.saturating_add(final_refund_amount);
        stats.count_refunded = stats.count_refunded.saturating_add(1);
        Self::set_counters(env, &stats);
    }

    /// Handle partial release from a Locked bounty.
    ///
    /// Called from `partial_release`. Reduces total_locked by the released amount
    /// and adds to total_released. If the bounty transitions to Released status
    /// after this partial release (remaining_amount == 0), the caller must also
    /// call `finalize_partial_release_to_released`.
    fn partial_release_from_locked(env: &Env, release_amount: i128) {
        let mut stats = Self::get_counters(env);
        stats.total_locked = stats.total_locked.saturating_sub(release_amount);
        stats.total_released = stats.total_released.saturating_add(release_amount);
        Self::set_counters(env, &stats);
    }

    /// Finalize a partial release that transitions the bounty to Released.
    ///
    /// Called from `partial_release` when remaining_amount reaches zero.
    /// Decrements locked count and increments released count.
    fn finalize_partial_release_to_released(env: &Env) {
        let mut stats = Self::get_counters(env);
        stats.count_locked = stats.count_locked.saturating_sub(1);
        stats.count_released = stats.count_released.saturating_add(1);
        Self::set_counters(env, &stats);
    }

    // ==================== END INCREMENTAL AGGREGATE COUNTERS ====================

    /// Initialize the contract with the admin address and the token address (XLM).
    pub fn init(env: Env, admin: Address, token: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);

        emit_bounty_initialized(
            &env,
            BountyEscrowInitialized {
                version: EVENT_VERSION_V2,
                admin,
                token,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(())
    }

    /// Calculate fee amount based on rate (in basis points)
    fn calculate_fee(amount: i128, fee_rate: i128) -> i128 {
        if fee_rate == 0 {
            return 0;
        }
        // Fee = (amount * fee_rate) / BASIS_POINTS
        // Using checked arithmetic to prevent overflow
        amount
            .checked_mul(fee_rate)
            .and_then(|x| x.checked_div(BASIS_POINTS))
            .unwrap_or(0)
    }

    /// Get fee configuration (internal helper)
    fn get_fee_config_internal(env: &Env) -> FeeConfig {
        env.storage()
            .instance()
            .get(&DataKey::FeeConfig)
            .unwrap_or_else(|| FeeConfig {
                lock_fee_rate: 0,
                release_fee_rate: 0,
                fee_recipient: env.storage().instance().get(&DataKey::Admin).unwrap(),
                fee_enabled: false,
            })
    }

    /// Update fee configuration (admin only)
    pub fn update_fee_config(
        env: Env,
        lock_fee_rate: Option<i128>,
        release_fee_rate: Option<i128>,
        fee_recipient: Option<Address>,
        fee_enabled: Option<bool>,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        // Check governance requirements
        Self::check_governance_requirements(&env)?;

        let mut fee_config = Self::get_fee_config_internal(&env);

        if let Some(rate) = lock_fee_rate {
            if rate < 0 || rate > MAX_FEE_RATE {
                return Err(Error::InvalidFeeRate);
            }
            fee_config.lock_fee_rate = rate;
        }

        if let Some(rate) = release_fee_rate {
            if rate < 0 || rate > MAX_FEE_RATE {
                return Err(Error::InvalidFeeRate);
            }
            fee_config.release_fee_rate = rate;
        }

        if let Some(recipient) = fee_recipient {
            fee_config.fee_recipient = recipient;
        }

        if let Some(enabled) = fee_enabled {
            fee_config.fee_enabled = enabled;
        }

        env.storage()
            .instance()
            .set(&DataKey::FeeConfig, &fee_config);

        events::emit_fee_config_updated(
            &env,
            events::FeeConfigUpdated {
                version: EVENT_VERSION_V2,
                lock_fee_rate: fee_config.lock_fee_rate,
                release_fee_rate: fee_config.release_fee_rate,
                fee_recipient: fee_config.fee_recipient.clone(),
                fee_enabled: fee_config.fee_enabled,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(())
    }

    /// Update pause flags (admin only)
    pub fn set_paused(
        env: Env,
        lock: Option<bool>,
        release: Option<bool>,
        refund: Option<bool>,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        // Check governance requirements
        Self::check_governance_requirements(&env)?;

        let mut flags = Self::get_pause_flags(&env);

        if let Some(paused) = lock {
            flags.lock_paused = paused;
            events::emit_pause_state_changed(
                &env,
                PauseStateChanged {
                    operation: symbol_short!("lock"),
                    paused,
                    admin: admin.clone(),
                },
            );
        }

        if let Some(paused) = release {
            flags.release_paused = paused;
            events::emit_pause_state_changed(
                &env,
                PauseStateChanged {
                    operation: symbol_short!("release"),
                    paused,
                    admin: admin.clone(),
                },
            );
        }

        if let Some(paused) = refund {
            flags.refund_paused = paused;
            events::emit_pause_state_changed(
                &env,
                PauseStateChanged {
                    operation: symbol_short!("refund"),
                    paused,
                    admin: admin.clone(),
                },
            );
        }

        env.storage().instance().set(&DataKey::PauseFlags, &flags);
        Ok(())
    }

    /// Set or clear the admin-only emergency pause kill switch.
    ///
    /// When enabled, every value-moving escrow entrypoint is halted with
    /// `Error::FundsPaused` while read/query functions remain available.
    pub fn set_emergency_pause(env: Env, paused: bool) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        // Check governance requirements before changing incident controls.
        Self::check_governance_requirements(&env)?;

        let mut flags = Self::get_pause_flags(&env);
        flags.global_paused = paused;

        events::emit_pause_state_changed(
            &env,
            PauseStateChanged {
                operation: symbol_short!("global"),
                paused,
                admin: admin.clone(),
            },
        );

        env.storage().instance().set(&DataKey::PauseFlags, &flags);
        Ok(())
    }

    /// Get current pause flags
    pub fn get_pause_flags(env: &Env) -> PauseFlags {
        env.storage()
            .instance()
            .get(&DataKey::PauseFlags)
            .unwrap_or(PauseFlags {
                global_paused: false,
                lock_paused: false,
                release_paused: false,
                refund_paused: false,
            })
    }

    /// Check if an operation is paused
    fn check_paused(env: &Env, operation: Symbol) -> bool {
        let flags = Self::get_pause_flags(env);
        if flags.global_paused {
            return true;
        }
        if operation == symbol_short!("lock") {
            return flags.lock_paused;
        } else if operation == symbol_short!("release") {
            return flags.release_paused;
        } else if operation == symbol_short!("refund") {
            return flags.refund_paused;
        }
        false
    }

    /// Get current fee configuration (view function)
    pub fn get_fee_config(env: Env) -> FeeConfig {
        Self::get_fee_config_internal(&env)
    }

    /// Update multisig configuration (admin only)
    pub fn update_multisig_config(
        env: Env,
        threshold_amount: i128,
        signers: Vec<Address>,
        required_signatures: u32,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        if required_signatures > signers.len() {
            return Err(Error::InvalidAmount);
        }

        let config = MultisigConfig {
            threshold_amount,
            signers,
            required_signatures,
        };

        env.storage()
            .instance()
            .set(&DataKey::MultisigConfig, &config);

        Ok(())
    }

    /// Get multisig configuration
    pub fn get_multisig_config(env: Env) -> MultisigConfig {
        env.storage()
            .instance()
            .get(&DataKey::MultisigConfig)
            .unwrap_or(MultisigConfig {
                threshold_amount: i128::MAX,
                signers: vec![&env],
                required_signatures: 0,
            })
    }

    /// Admin-focused audit view of core configuration.
    ///
    /// This view returns a compact, versioned snapshot combining admin,
    /// token, fee configuration, pause flags, governance wiring, and
    /// key risk controls such as claim window and amount policy.
    /// It is designed for off-chain dashboards and monitoring systems
    /// that need a single stable entrypoint for configuration audits.
    pub fn get_admin_audit_view(env: Env) -> AdminConfigSnapshot {
        // If the contract is not initialized yet, return a conservative
        // snapshot rooted in the current contract address so callers
        // never see malformed addresses.
        if !env.storage().instance().has(&DataKey::Admin) {
            let contract_addr = env.current_contract_address();
            let fee_config = FeeConfig {
                lock_fee_rate: 0,
                release_fee_rate: 0,
                fee_recipient: contract_addr.clone(),
                fee_enabled: false,
            };
            let pause_flags = PauseFlags {
                global_paused: false,
                lock_paused: false,
                release_paused: false,
                refund_paused: false,
            };

            return AdminConfigSnapshot {
                version: 1,
                admin: contract_addr.clone(),
                token: contract_addr,
                fee_config,
                pause_flags,
                governance_contract: None,
                min_governance_version: 0,
                claim_window: 0,
                has_amount_policy: false,
                min_lock_amount: 0,
                max_lock_amount: 0,
            };
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let fee_config = Self::get_fee_config_internal(&env);
        let pause_flags = Self::get_pause_flags(&env);

        let governance_contract = Self::get_governance_contract(env.clone());
        let min_governance_version = Self::get_min_governance_version(env.clone());

        let claim_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ClaimWindow)
            .unwrap_or(0);

        let mut has_amount_policy = false;
        let mut min_lock_amount: i128 = 0;
        let mut max_lock_amount: i128 = 0;

        if let Some((min_amount, max_amount)) = env
            .storage()
            .instance()
            .get::<DataKey, (i128, i128)>(&DataKey::AmountPolicy)
        {
            has_amount_policy = true;
            min_lock_amount = min_amount;
            max_lock_amount = max_amount;
        }

        AdminConfigSnapshot {
            version: 1,
            admin,
            token,
            fee_config,
            pause_flags,
            governance_contract,
            min_governance_version,
            claim_window,
            has_amount_policy,
            min_lock_amount,
            max_lock_amount,
        }
    }

    /// Approve release for large amount (requires multisig)
    pub fn approve_large_release(
        env: Env,
        bounty_id: u64,
        contributor: Address,
        approver: Address,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let multisig_config: MultisigConfig = Self::get_multisig_config(env.clone());

        let mut is_signer = false;
        for signer in multisig_config.signers.iter() {
            if signer == approver {
                is_signer = true;
                break;
            }
        }

        if !is_signer {
            return Err(Error::Unauthorized);
        }

        approver.require_auth();

        let approval_key = DataKey::ReleaseApproval(bounty_id);
        let mut approval: ReleaseApproval = env
            .storage()
            .persistent()
            .get(&approval_key)
            .unwrap_or(ReleaseApproval {
                bounty_id,
                contributor: contributor.clone(),
                approvals: vec![&env],
            });

        for existing in approval.approvals.iter() {
            if existing == approver {
                return Ok(());
            }
        }

        approval.approvals.push_back(approver.clone());
        env.storage().persistent().set(&approval_key, &approval);

        events::emit_approval_added(
            &env,
            events::ApprovalAdded {
                version: EVENT_VERSION_V2,
                bounty_id,
                contributor: contributor.clone(),
                approver,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(())
    }

    /// Lock funds for a specific bounty.
    pub fn lock_funds(
        env: Env,
        depositor: Address,
        bounty_id: u64,
        amount: i128,
        deadline: u64,
    ) -> Result<(), Error> {
        // Apply rate limiting
        anti_abuse::check_rate_limit(&env, depositor.clone());

        if Self::check_paused(&env, symbol_short!("lock")) {
            return Err(Error::FundsPaused);
        }

        // Check circuit breaker before proceeding
        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }

        let _start = env.ledger().timestamp();
        let _caller = depositor.clone();

        // Verify depositor authorization
        depositor.require_auth();

        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        if env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyExists);
        }

        // Enforce min/max amount policy if one has been configured (Issue #62).
        // When no policy is set this block is skipped entirely, preserving
        // backward-compatible behaviour for callers that never call set_amount_policy.
        if let Some((min_amount, max_amount)) = env
            .storage()
            .instance()
            .get::<DataKey, (i128, i128)>(&DataKey::AmountPolicy)
        {
            if amount < min_amount {
                return Err(Error::AmountBelowMinimum);
            }
            if amount > max_amount {
                return Err(Error::AmountAboveMaximum);
            }
        }

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);

        // Transfer funds from depositor to contract
        client.transfer(&depositor, &env.current_contract_address(), &amount);

        let escrow = Escrow {
            depositor: depositor.clone(),
            amount,
            status: EscrowStatus::Locked,
            deadline,
            refund_history: vec![&env],
            remaining_amount: amount,
        };

        // Extend the TTL of the storage entry to ensure it lives long enough
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(bounty_id), &escrow);
        Self::bump_escrow_ttl(&env, bounty_id);

        // Update indexes
        let mut index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));
        index.push_back(bounty_id);
        env.storage()
            .persistent()
            .set(&DataKey::EscrowIndex, &index);

        let mut depositor_index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::DepositorIndex(depositor.clone()))
            .unwrap_or(Vec::new(&env));
        depositor_index.push_back(bounty_id);
        env.storage().persistent().set(
            &DataKey::DepositorIndex(depositor.clone()),
            &depositor_index,
        );
        Self::bump_escrow_index_ttl(&env, depositor.clone());

        // Initialize analytics for this bounty
        let timestamp = env.ledger().timestamp();
        init_bounty_analytics(&env, bounty_id, amount, timestamp);

        // Update incremental aggregate counters
        Self::increment_locked(&env, amount);

        // Emit state transition event
        emit_bounty_state_transitioned(
            &env,
            BountyStateTransitioned {
                version: analytics::ANALYTICS_VERSION_V1,
                bounty_id,
                previous_state: symbol_short!("new"),
                new_state: symbol_short!("locked"),
                amount,
                actor: depositor.clone(),
                timestamp,
            },
        );

        // Emit activity event
        emit_bounty_activity(
            &env,
            BountyActivityEvent {
                version: analytics::ANALYTICS_VERSION_V1,
                bounty_id,
                activity_type: symbol_short!("created"),
                amount,
                timestamp,
            },
        );

        // Emit traditional event for backward compatibility
        emit_funds_locked(
            &env,
            FundsLocked {
                version: EVENT_VERSION_V2,
                bounty_id,
                amount,
                depositor: depositor.clone(),
                deadline,
            },
        );

        Ok(())
    }

    /// Release funds to the contributor.
    /// Only the admin (backend) can authorize this.
    /// If governance is configured, the linked governance version must meet
    /// the configured minimum before this value transfer can proceed.
    pub fn release_funds(env: Env, bounty_id: u64, contributor: Address) -> Result<(), Error> {
        if Self::check_paused(&env, symbol_short!("release")) {
            return Err(Error::FundsPaused);
        }

        // Check circuit breaker before proceeding
        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }

        Self::check_governance_requirements(&env)?;

        // --- All validation BEFORE the reentrancy guard so early returns never
        //     leak the guard flag. ---

        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        // Dispute protection: if a pending claim exists for this bounty,
        // the dispute must be resolved (claim or cancel) before any direct release.
        if env
            .storage()
            .persistent()
            .has(&DataKey::PendingClaim(bounty_id))
        {
            return Err(Error::RefundNotApproved);
        }

        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();

        if escrow.status != EscrowStatus::Locked {
            return Err(Error::FundsNotLocked);
        }

        let _start = env.ledger().timestamp();

        // --- Reentrancy guard: set after all validation so it cannot leak. ---
        if env.storage().instance().has(&DataKey::ReentrancyGuard) {
            panic!("Reentrancy detected");
        }
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyGuard, &true);

        // --- Effects-before-interactions: update all state BEFORE the external
        //     token transfer so a reentrant call sees the already-released status
        //     and cannot double-spend. ---
        let release_amount = escrow.amount;
        escrow.status = EscrowStatus::Released;
        // Zero remaining_amount to maintain SAC ≡ Σ remaining_amount invariant.
        escrow.remaining_amount = 0;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(bounty_id), &escrow);
        Self::bump_escrow_ttl(&env, bounty_id);

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);

        // External call: transfer funds to contributor (state already committed above).
        client.transfer(
            &env.current_contract_address(),
            &contributor,
            &release_amount,
        );

        let timestamp = env.ledger().timestamp();

        // Update analytics
        update_analytics_on_release(&env, bounty_id, release_amount, timestamp);

        // Update incremental aggregate counters
        Self::transition_locked_to_released(&env, release_amount);

        // Emit state transition event
        emit_bounty_state_transitioned(
            &env,
            BountyStateTransitioned {
                version: analytics::ANALYTICS_VERSION_V1,
                bounty_id,
                previous_state: symbol_short!("locked"),
                new_state: symbol_short!("released"),
                amount: release_amount,
                actor: admin.clone(),
                timestamp,
            },
        );

        // Emit activity event
        emit_bounty_activity(
            &env,
            BountyActivityEvent {
                version: analytics::ANALYTICS_VERSION_V1,
                bounty_id,
                activity_type: symbol_short!("released"),
                amount: release_amount,
                timestamp,
            },
        );

        emit_funds_released(
            &env,
            FundsReleased {
                version: EVENT_VERSION_V2,
                bounty_id,
                amount: release_amount,
                recipient: contributor.clone(),
                timestamp,
            },
        );

        // Clear reentrancy guard
        env.storage().instance().remove(&DataKey::ReentrancyGuard);

        Ok(())
    }

    /// Set the claim window duration (admin only).
    /// claim_window: seconds beneficiary has to claim after release is authorized.
    pub fn set_claim_window(env: Env, claim_window: u64) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::ClaimWindow, &claim_window);
        Ok(())
    }

    /// Authorize a release as a pending claim instead of immediate transfer.
    /// Admin calls this instead of release_funds when claim period is active.
    /// Beneficiary must call claim() within the window to receive funds.
    pub fn authorize_claim(env: Env, bounty_id: u64, recipient: Address) -> Result<(), Error> {
        if Self::check_paused(&env, symbol_short!("release")) {
            return Err(Error::FundsPaused);
        }
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();

        if escrow.status != EscrowStatus::Locked {
            return Err(Error::FundsNotLocked);
        }

        // Guard: reject if a non-expired, unclaimed pending claim already exists.
        // This prevents silently overwriting a live claim to redirect funds.
        if let Some(existing) = env
            .storage()
            .persistent()
            .get::<_, ClaimRecord>(&DataKey::PendingClaim(bounty_id))
        {
            let now = env.ledger().timestamp();
            if !existing.claimed && now <= existing.expires_at {
                return Err(Error::PendingClaimExists);
            }
        }

        let now = env.ledger().timestamp();
        let claim_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ClaimWindow)
            .unwrap_or(0);
        let claim = ClaimRecord {
            bounty_id,
            recipient: recipient.clone(),
            amount: escrow.amount,
            expires_at: now.saturating_add(claim_window),
            claimed: false,
        };

        env.storage()
            .persistent()
            .set(&DataKey::PendingClaim(bounty_id), &claim);

        emit_claim_created(
            &env,
            ClaimCreated {
                version: EVENT_VERSION_V2,
                bounty_id,
                recipient,
                amount: escrow.amount,
                expires_at: claim.expires_at,
            },
        );
        Ok(())
    }

    /// Beneficiary calls this to claim their authorized funds within the window.
    pub fn claim(env: Env, bounty_id: u64) -> Result<(), Error> {
        if Self::check_paused(&env, symbol_short!("release")) {
            return Err(Error::FundsPaused);
        }

        // Check circuit breaker before proceeding
        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }

        // --- All validation BEFORE the reentrancy guard so early returns never
        //     leak the guard flag. ---

        if !env
            .storage()
            .persistent()
            .has(&DataKey::PendingClaim(bounty_id))
        {
            return Err(Error::BountyNotFound);
        }
        let mut claim: ClaimRecord = env
            .storage()
            .persistent()
            .get(&DataKey::PendingClaim(bounty_id))
            .unwrap();

        claim.recipient.require_auth();

        let now = env.ledger().timestamp();
        if now > claim.expires_at {
            return Err(Error::ClaimExpired);
        }
        if claim.claimed {
            return Err(Error::FundsNotLocked);
        }

        // --- Reentrancy guard: set after all validation so it cannot leak. ---
        // Defense in depth: token transfer is an external call. Block nested
        // entry into claim/release paths before interacting with the token.
        if env.storage().instance().has(&DataKey::ReentrancyGuard) {
            panic!("Reentrancy detected");
        }
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyGuard, &true);

        // --- Effects-before-interactions: update all state BEFORE the external
        //     token transfer. A reentrant call sees claim.claimed==true and
        //     escrow.status==Released so it cannot double-spend. ---

        let claim_amount = claim.amount;

        // Update escrow status
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();
        escrow.status = EscrowStatus::Released;
        // Zero remaining_amount to maintain invariant
        escrow.remaining_amount = 0;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(bounty_id), &escrow);
        Self::bump_escrow_ttl(&env, bounty_id);

        // Update incremental aggregate counters
        Self::transition_locked_to_released(&env, claim_amount);

        claim.claimed = true;
        env.storage()
            .persistent()
            .set(&DataKey::PendingClaim(bounty_id), &claim);

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);

        // External call: transfer funds to recipient (state already committed above).
        client.transfer(
            &env.current_contract_address(),
            &claim.recipient,
            &claim_amount,
        );

        emit_claim_executed(
            &env,
            ClaimExecuted {
                version: EVENT_VERSION_V2,
                bounty_id,
                recipient: claim.recipient.clone(),
                amount: claim_amount,
                claimed_at: now,
            },
        );
        env.storage().instance().remove(&DataKey::ReentrancyGuard);
        Ok(())
    }

    /// Admin can cancel an expired or unwanted pending claim, returning escrow to Locked.
    pub fn cancel_pending_claim(env: Env, bounty_id: u64) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        if !env
            .storage()
            .persistent()
            .has(&DataKey::PendingClaim(bounty_id))
        {
            return Err(Error::BountyNotFound);
        }
        let claim: ClaimRecord = env
            .storage()
            .persistent()
            .get(&DataKey::PendingClaim(bounty_id))
            .unwrap();

        if claim.claimed {
            return Err(Error::FundsNotLocked);
        }

        let cancelled_at = env.ledger().timestamp();
        let reason = if cancelled_at > claim.expires_at {
            symbol_short!("expired")
        } else {
            symbol_short!("manual")
        };

        env.storage()
            .persistent()
            .remove(&DataKey::PendingClaim(bounty_id));

        emit_claim_cancelled(
            &env,
            ClaimCancelled {
                version: EVENT_VERSION_V2,
                bounty_id,
                recipient: claim.recipient,
                amount: claim.amount,
                cancelled_at,
                cancelled_by: admin,
                reason,
            },
        );
        Ok(())
    }

    /// View: get pending claim for a bounty.
    pub fn get_pending_claim(env: Env, bounty_id: u64) -> Result<ClaimRecord, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::PendingClaim(bounty_id))
            .ok_or(Error::BountyNotFound)
    }

    /// Approve a refund before deadline (admin only).
    /// This allows early refunds with admin approval.
    pub fn approve_refund(
        env: Env,
        bounty_id: u64,
        amount: i128,
        recipient: Address,
        mode: RefundMode,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();
        Self::bump_escrow_ttl(&env, bounty_id);

        if escrow.status != EscrowStatus::Locked && escrow.status != EscrowStatus::PartiallyRefunded
        {
            return Err(Error::FundsNotLocked);
        }

        if amount <= 0 || amount > escrow.remaining_amount {
            return Err(Error::InvalidAmount);
        }

        let approval = RefundApproval {
            bounty_id,
            amount,
            recipient: recipient.clone(),
            mode: mode.clone(),
            approved_by: admin.clone(),
            approved_at: env.ledger().timestamp(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::RefundApproval(bounty_id), &approval);

        Ok(())
    }

    /// Release a partial amount of the locked funds to the contributor.
    /// Only the admin (backend) can authorize this.
    ///
    /// - `payout_amount` must be > 0 and <= `remaining_amount`.
    /// - `remaining_amount` is decremented by `payout_amount` after each call.
    /// - When `remaining_amount` reaches 0 the escrow status is set to Released.
    /// - The bounty stays Locked while any funds remain unreleased.
    /// - If governance is configured, its version must satisfy the configured
    ///   minimum before any partial payout is transferred.
    pub fn partial_release(
        env: Env,
        bounty_id: u64,
        contributor: Address,
        payout_amount: i128,
    ) -> Result<(), Error> {
        if Self::check_paused(&env, symbol_short!("release")) {
            return Err(Error::FundsPaused);
        }

        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        // Check circuit breaker before proceeding
        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }

        Self::check_governance_requirements(&env)?;

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        // Dispute protection: block partial releases while a dispute (pending claim) is open.
        if env
            .storage()
            .persistent()
            .has(&DataKey::PendingClaim(bounty_id))
        {
            return Err(Error::RefundNotApproved);
        }

        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();

        if escrow.status != EscrowStatus::Locked {
            return Err(Error::FundsNotLocked);
        }

        // Guard: zero or negative payout makes no sense and would corrupt state
        if payout_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        // Guard: prevent overpayment — payout cannot exceed what is still owed
        if payout_amount > escrow.remaining_amount {
            return Err(Error::InsufficientFunds);
        }

        // Defense in depth: token transfer is an external call. Block nested
        // entry into claim/release paths before interacting with the token.
        if env.storage().instance().has(&DataKey::ReentrancyGuard) {
            panic!("Reentrancy detected");
        }
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyGuard, &true);

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);

        // Transfer only the requested partial amount to the contributor
        client.transfer(
            &env.current_contract_address(),
            &contributor,
            &payout_amount,
        );

        // Decrement remaining; this is always an exact integer subtraction — no rounding
        escrow.remaining_amount -= payout_amount;

        // Automatically transition to Released once fully paid out
        let transitioned_to_released = escrow.remaining_amount == 0;
        if transitioned_to_released {
            escrow.status = EscrowStatus::Released;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(bounty_id), &escrow);
        Self::bump_escrow_ttl(&env, bounty_id);

        // Update incremental aggregate counters
        Self::partial_release_from_locked(&env, payout_amount);
        if transitioned_to_released {
            Self::finalize_partial_release_to_released(&env);
        }

        events::emit_funds_released(
            &env,
            FundsReleased {
                version: EVENT_VERSION_V2,
                bounty_id,
                amount: payout_amount,
                recipient: contributor.clone(),
                timestamp: env.ledger().timestamp(),
            },
        );

        env.storage().instance().remove(&DataKey::ReentrancyGuard);
        Ok(())
    }

    fn load_refundable_escrow(env: &Env, bounty_id: u64) -> Result<Escrow, Error> {
        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }

        // Dispute protection: an open pending claim represents an unresolved dispute.
        // Refunds are blocked until the claim is explicitly cancelled.
        if env
            .storage()
            .persistent()
            .has(&DataKey::PendingClaim(bounty_id))
        {
            return Err(Error::RefundNotApproved);
        }

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();

        if escrow.status != EscrowStatus::Locked && escrow.status != EscrowStatus::PartiallyRefunded
        {
            return Err(Error::FundsNotLocked);
        }

        Ok(escrow)
    }

    fn validate_unique_bounty_ids(bounty_ids: &Vec<u64>) -> Result<(), Error> {
        for bounty_id in bounty_ids.iter() {
            let mut count = 0u32;
            for other_id in bounty_ids.iter() {
                if other_id == bounty_id {
                    count += 1;
                }
            }
            if count > 1 {
                return Err(Error::DuplicateBountyId);
            }
        }
        Ok(())
    }

    /// Refund funds to the original depositor if the deadline has passed.
    /// Refunds the full remaining_amount (accounts for any prior partial releases).
    /// If governance is configured, the linked governance version must meet
    /// the configured minimum before funds can be refunded.
    pub fn refund(env: Env, bounty_id: u64) -> Result<(), Error> {
        if Self::check_paused(&env, symbol_short!("refund")) {
            return Err(Error::FundsPaused);
        }

        // Check circuit breaker before proceeding
        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }

        Self::check_governance_requirements(&env)?;

        let mut escrow = Self::load_refundable_escrow(&env, bounty_id)?;

        let now = env.ledger().timestamp();
        let approval_key = DataKey::RefundApproval(bounty_id);
        let approval: Option<RefundApproval> = env.storage().persistent().get(&approval_key);

        // Refund is allowed if:
        // 1. Deadline has passed (returns full amount to depositor)
        // 2. An administrative approval exists (can be early, partial, and to custom recipient)
        if now < escrow.deadline && approval.is_none() {
            return Err(Error::DeadlineNotPassed);
        }

        let (refund_amount, refund_to, is_full) = if let Some(app) = approval.clone() {
            let full = app.mode == RefundMode::Full || app.amount >= escrow.remaining_amount;
            (app.amount, app.recipient, full)
        } else {
            // Standard refund after deadline
            (escrow.remaining_amount, escrow.depositor.clone(), true)
        };

        if refund_amount <= 0 || refund_amount > escrow.remaining_amount {
            return Err(Error::InvalidAmount);
        }

        if env.storage().instance().has(&DataKey::ReentrancyGuard) {
            panic!("Reentrancy detected");
        }
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyGuard, &true);

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);

        // Transfer the calculated refund amount to the designated recipient
        client.transfer(&env.current_contract_address(), &refund_to, &refund_amount);

        // Track original status to determine counter update strategy
        let original_status = escrow.status.clone();

        // Update escrow state: subtract the amount exactly refunded
        escrow.remaining_amount -= refund_amount;
        if is_full || escrow.remaining_amount == 0 {
            escrow.status = EscrowStatus::Refunded;
        } else {
            escrow.status = EscrowStatus::PartiallyRefunded;
        }

        // Add to refund history
        escrow.refund_history.push_back(RefundRecord {
            amount: refund_amount,
            recipient: refund_to.clone(),
            timestamp: now,
            mode: if is_full {
                RefundMode::Full
            } else {
                RefundMode::Partial
            },
        });

        // Save updated escrow
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(bounty_id), &escrow);
        Self::bump_escrow_ttl(&env, bounty_id);

        // Update analytics
        update_analytics_on_refund(&env, bounty_id, refund_amount, now);

        // Update incremental aggregate counters
        match (original_status, escrow.status.clone()) {
            (EscrowStatus::Locked, EscrowStatus::Refunded) => {
                // Full refund from locked
                Self::transition_locked_to_refunded(&env, refund_amount);
            }
            (EscrowStatus::Locked, EscrowStatus::PartiallyRefunded) => {
                // Partial refund from locked
                Self::partial_refund_from_locked(&env, refund_amount);
            }
            (EscrowStatus::PartiallyRefunded, EscrowStatus::Refunded) => {
                // Final refund completing a partially refunded bounty
                Self::transition_partially_refunded_to_refunded(&env, refund_amount);
            }
            (EscrowStatus::PartiallyRefunded, EscrowStatus::PartiallyRefunded) => {
                // Additional partial refund (still partially refunded)
                Self::partial_refund_from_locked(&env, refund_amount);
            }
            _ => {
                // Unexpected state transition - should not happen due to validation in load_refundable_escrow
            }
        }

        // Emit state transition event
        let new_state = if is_full || escrow.remaining_amount == 0 {
            symbol_short!("refunded")
        } else {
            symbol_short!("partial_r")
        };
        emit_bounty_state_transitioned(
            &env,
            BountyStateTransitioned {
                version: analytics::ANALYTICS_VERSION_V1,
                bounty_id,
                previous_state: symbol_short!("locked"),
                new_state,
                amount: refund_amount,
                actor: refund_to.clone(),
                timestamp: now,
            },
        );

        // Emit activity event
        emit_bounty_activity(
            &env,
            BountyActivityEvent {
                version: analytics::ANALYTICS_VERSION_V1,
                bounty_id,
                activity_type: if is_full {
                    symbol_short!("refunded")
                } else {
                    symbol_short!("part_ref")
                },
                amount: refund_amount,
                timestamp: now,
            },
        );

        // Remove approval after successful execution
        if approval.is_some() {
            env.storage().persistent().remove(&approval_key);
        }

        emit_funds_refunded(
            &env,
            FundsRefunded {
                version: EVENT_VERSION_V2,
                bounty_id,
                amount: refund_amount,
                refund_to: refund_to.clone(),
                timestamp: now,
            },
        );

        env.storage().instance().remove(&DataKey::ReentrancyGuard);

        Ok(())
    }

    /// Sweep a bounded batch of expired bounties and refund each depositor.
    ///
    /// This helper follows the same post-deadline refund rules as `refund`:
    /// every bounty must exist, have no pending claim, be locked or partially
    /// refunded, and have `deadline <= now`. The batch is validated before any
    /// transfer, so one invalid entry rejects the whole sweep.
    /// If governance is configured, the linked governance version must meet
    /// the configured minimum before any expired bounty is swept.
    pub fn sweep_expired_refunds(env: Env, bounty_ids: Vec<u64>) -> Result<u32, Error> {
        if Self::check_paused(&env, symbol_short!("refund")) {
            return Err(Error::FundsPaused);
        }

        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }

        Self::check_governance_requirements(&env)?;

        let batch_size = bounty_ids.len() as u32;
        if batch_size == 0 || batch_size > MAX_BATCH_SIZE {
            return Err(Error::InvalidBatchSize);
        }
        Self::validate_unique_bounty_ids(&bounty_ids)?;

        let now = env.ledger().timestamp();
        for bounty_id in bounty_ids.iter() {
            let escrow = Self::load_refundable_escrow(&env, bounty_id)?;
            if now < escrow.deadline {
                return Err(Error::DeadlineNotPassed);
            }
            if escrow.remaining_amount <= 0 {
                return Err(Error::InvalidAmount);
            }
        }

        if env.storage().instance().has(&DataKey::ReentrancyGuard) {
            panic!("Reentrancy detected");
        }
        env.storage()
            .instance()
            .set(&DataKey::ReentrancyGuard, &true);

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);
        let contract_address = env.current_contract_address();
        let mut refunded_count = 0u32;

        for bounty_id in bounty_ids.iter() {
            let mut escrow: Escrow = env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(bounty_id))
                .unwrap();
            let refund_amount = escrow.remaining_amount;
            let refund_to = escrow.depositor.clone();
            let previous_status = escrow.status.clone();
            let previous_state = if escrow.status == EscrowStatus::PartiallyRefunded {
                symbol_short!("partial_r")
            } else {
                symbol_short!("locked")
            };

            client.transfer(&contract_address, &refund_to, &refund_amount);

            escrow.remaining_amount = 0;
            escrow.status = EscrowStatus::Refunded;
            escrow.refund_history.push_back(RefundRecord {
                amount: refund_amount,
                recipient: refund_to.clone(),
                timestamp: now,
                mode: RefundMode::Full,
            });

            // Update incremental aggregate counters
            if previous_status == EscrowStatus::Locked {
                Self::transition_locked_to_refunded(&env, refund_amount);
            } else if previous_status == EscrowStatus::PartiallyRefunded {
                Self::transition_partially_refunded_to_refunded(&env, refund_amount);
            }

            env.storage()
                .persistent()
                .set(&DataKey::Escrow(bounty_id), &escrow);
            Self::bump_escrow_ttl(&env, bounty_id);

            update_analytics_on_refund(&env, bounty_id, refund_amount, now);

            emit_bounty_state_transitioned(
                &env,
                BountyStateTransitioned {
                    version: analytics::ANALYTICS_VERSION_V1,
                    bounty_id,
                    previous_state,
                    new_state: symbol_short!("refunded"),
                    amount: refund_amount,
                    actor: refund_to.clone(),
                    timestamp: now,
                },
            );

            emit_bounty_activity(
                &env,
                BountyActivityEvent {
                    version: analytics::ANALYTICS_VERSION_V1,
                    bounty_id,
                    activity_type: symbol_short!("refunded"),
                    amount: refund_amount,
                    timestamp: now,
                },
            );

            emit_bounty_expired(
                &env,
                BountyExpired {
                    version: EVENT_VERSION_V2,
                    bounty_id,
                    depositor: refund_to.clone(),
                    amount: refund_amount,
                    deadline: escrow.deadline,
                    expired_at: now,
                },
            );

            emit_funds_refunded(
                &env,
                FundsRefunded {
                    version: EVENT_VERSION_V2,
                    bounty_id,
                    amount: refund_amount,
                    refund_to,
                    timestamp: now,
                },
            );

            refunded_count += 1;
        }

        env.storage().instance().remove(&DataKey::ReentrancyGuard);

        Ok(refunded_count)
    }

    /// view function to get escrow info
    pub fn get_escrow_info(env: Env, bounty_id: u64) -> Result<Escrow, Error> {
        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }
        let escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();
        Self::bump_escrow_ttl(&env, bounty_id);
        Ok(escrow)
    }

    /// view function to get contract balance of the token
    pub fn get_balance(env: Env) -> Result<i128, Error> {
        if !env.storage().instance().has(&DataKey::Token) {
            return Err(Error::NotInitialized);
        }
        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);
        Ok(client.balance(&env.current_contract_address()))
    }

    /// Query escrows with filtering and pagination
    /// Pass 0 for min values and i128::MAX/u64::MAX for max values to disable those filters
    pub fn query_escrows_by_status(
        env: Env,
        status: EscrowStatus,
        offset: u32,
        limit: u32,
    ) -> Vec<EscrowWithId> {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));
        let mut results = Vec::new(&env);
        let mut count = 0u32;
        let mut skipped = 0u32;

        for i in 0..index.len() {
            if count >= limit {
                break;
            }

            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                if escrow.status == status {
                    if skipped < offset {
                        skipped += 1;
                        continue;
                    }
                    results.push_back(EscrowWithId { bounty_id, escrow });
                    count += 1;
                }
            }
        }
        results
    }

    /// Query escrows with amount range filtering
    pub fn query_escrows_by_amount(
        env: Env,
        min_amount: i128,
        max_amount: i128,
        offset: u32,
        limit: u32,
    ) -> Vec<EscrowWithId> {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));
        let mut results = Vec::new(&env);
        let mut count = 0u32;
        let mut skipped = 0u32;

        for i in 0..index.len() {
            if count >= limit {
                break;
            }

            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                if escrow.amount >= min_amount && escrow.amount <= max_amount {
                    if skipped < offset {
                        skipped += 1;
                        continue;
                    }
                    results.push_back(EscrowWithId { bounty_id, escrow });
                    count += 1;
                }
            }
        }
        results
    }

    /// Query escrows with deadline range filtering
    pub fn query_escrows_by_deadline(
        env: Env,
        min_deadline: u64,
        max_deadline: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<EscrowWithId> {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));
        let mut results = Vec::new(&env);
        let mut count = 0u32;
        let mut skipped = 0u32;

        for i in 0..index.len() {
            if count >= limit {
                break;
            }

            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                if escrow.deadline >= min_deadline && escrow.deadline <= max_deadline {
                    if skipped < offset {
                        skipped += 1;
                        continue;
                    }
                    results.push_back(EscrowWithId { bounty_id, escrow });
                    count += 1;
                }
            }
        }
        results
    }

    /// Query escrows by depositor
    pub fn query_escrows_by_depositor(
        env: Env,
        depositor: Address,
        offset: u32,
        limit: u32,
    ) -> Vec<EscrowWithId> {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::DepositorIndex(depositor))
            .unwrap_or(Vec::new(&env));
        let mut results = Vec::new(&env);
        let start = offset.min(index.len());
        let end = (offset + limit).min(index.len());

        for i in start..end {
            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                results.push_back(EscrowWithId { bounty_id, escrow });
            }
        }
        results
    }

    /// Query escrows with composite filtering and pagination.
    /// This function enables rich querying by combining multiple filter criteria.
    ///
    /// # Performance Optimization
    /// When a depositor filter is specified (has_depositor_filter = true), this function
    /// uses the DepositorIndex for O(n) performance where n = depositor's escrows, rather
    /// than scanning all escrows O(N). For queries without a depositor filter, it scans
    /// the global EscrowIndex.
    ///
    /// # Filter Semantics
    /// - All active filters are combined with AND logic
    /// - Inactive filters (has_*_filter = false) are ignored
    /// - Amount filters use sentinel values: 0 for no min, i128::MAX for no max
    /// - Deadline filters use sentinel values: 0 for no min, u64::MAX for no max
    /// - Amount and deadline comparisons are inclusive
    ///
    /// # Pagination
    /// - offset: Number of matching records to skip
    /// - limit: Maximum number of records to return
    /// - Pagination is stable and works correctly with any filter combination
    ///
    /// # Security Notes
    /// - Read-only query function - no state modifications
    /// - Does not require authentication
    /// - Safe for public use - only exposes already-public escrow data
    pub fn query_escrows(
        env: Env,
        filter: EscrowQueryFilter,
        offset: u32,
        limit: u32,
    ) -> Vec<EscrowWithId> {
        // Optimization: use depositor index when depositor filter is active
        let index: Vec<u64> = if filter.has_depositor_filter {
            env.storage()
                .persistent()
                .get(&DataKey::DepositorIndex(filter.depositor.clone()))
                .unwrap_or(Vec::new(&env))
        } else {
            env.storage()
                .persistent()
                .get(&DataKey::EscrowIndex)
                .unwrap_or(Vec::new(&env))
        };

        let mut results = Vec::new(&env);
        let mut count = 0u32;
        let mut skipped = 0u32;

        for i in 0..index.len() {
            if count >= limit {
                break;
            }

            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                // Apply all active filters
                let mut matches = true;

                // Status filter
                if filter.has_status_filter {
                    if escrow.status != filter.status {
                        matches = false;
                    }
                }

                // Depositor filter (only needed if using global index)
                if filter.has_depositor_filter {
                    // When using depositor index, this is redundant but kept for correctness
                    // when global index is used
                    if escrow.depositor != filter.depositor {
                        matches = false;
                    }
                }

                // Min amount filter (0 means no minimum)
                if filter.min_amount > 0 && escrow.amount < filter.min_amount {
                    matches = false;
                }

                // Max amount filter (i128::MAX means no maximum)
                if filter.max_amount < i128::MAX && escrow.amount > filter.max_amount {
                    matches = false;
                }

                // Min deadline filter (0 means no minimum)
                if filter.min_deadline > 0 && escrow.deadline < filter.min_deadline {
                    matches = false;
                }

                // Max deadline filter (u64::MAX means no maximum)
                if filter.max_deadline < u64::MAX && escrow.deadline > filter.max_deadline {
                    matches = false;
                }

                if matches {
                    if skipped < offset {
                        skipped += 1;
                        continue;
                    }
                    results.push_back(EscrowWithId { bounty_id, escrow });
                    count += 1;
                }
            }
        }
        results
    }

    /// Get aggregate statistics from O(1) incremental counters.
    ///
    /// This function reads maintained counters that are updated on every state transition
    /// (lock, release, refund, partial_release). Provides constant-time aggregate queries.
    ///
    /// For verification purposes, use `get_aggregate_stats_full_scan` to perform a
    /// ground-truth comparison.
    pub fn get_aggregate_stats(env: Env) -> AggregateStats {
        Self::get_counters(&env)
    }

    /// Get aggregate statistics via full O(N) scan for reconciliation and testing.
    ///
    /// This function performs a complete scan of all escrows to calculate aggregate
    /// statistics from scratch. Use this to verify the incremental counters are accurate
    /// or when counters need to be rebuilt.
    ///
    /// **Performance:** O(N) where N is the number of bounties. Not suitable for
    /// production queries at scale.
    pub fn get_aggregate_stats_full_scan(env: Env) -> AggregateStats {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));
        let mut stats = AggregateStats {
            total_locked: 0,
            total_released: 0,
            total_refunded: 0,
            count_locked: 0,
            count_released: 0,
            count_refunded: 0,
        };

        for i in 0..index.len() {
            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                let mut refund_sum = 0i128;
                for record in escrow.refund_history.iter() {
                    refund_sum += record.amount;
                }
                let released_sum = escrow.amount - escrow.remaining_amount - refund_sum;

                stats.total_released += released_sum;
                stats.total_refunded += refund_sum;

                match escrow.status {
                    EscrowStatus::Locked | EscrowStatus::PartiallyRefunded => {
                        stats.total_locked += escrow.remaining_amount;
                        stats.count_locked += 1;
                    }
                    EscrowStatus::Released => {
                        stats.count_released += 1;
                    }
                    EscrowStatus::Refunded => {
                        stats.count_refunded += 1;
                    }
                }
            }
        }
        stats
    }

    /// Get total count of escrows
    pub fn get_escrow_count(env: Env) -> u32 {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));
        index.len()
    }

    /// Set the minimum and maximum allowed lock amount (admin only).
    ///
    /// Once set, any call to lock_funds with an amount outside [min_amount, max_amount]
    /// will be rejected with AmountBelowMinimum or AmountAboveMaximum respectively.
    /// The policy can be updated at any time by the admin; new limits take effect
    /// immediately for subsequent lock_funds calls.
    ///
    /// Passing min_amount == max_amount restricts locking to a single exact value.
    /// min_amount must not exceed max_amount — the call panics if this invariant
    /// is violated.
    pub fn set_amount_policy(
        env: Env,
        caller: Address,
        min_amount: i128,
        max_amount: i128,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != admin {
            return Err(Error::Unauthorized);
        }
        admin.require_auth();

        if min_amount > max_amount {
            panic!("invalid policy: min_amount cannot exceed max_amount");
        }

        // Persist the policy so lock_funds can enforce it on every subsequent call.
        env.storage()
            .instance()
            .set(&DataKey::AmountPolicy, &(min_amount, max_amount));

        Ok(())
    }

    /// Get escrow IDs by status
    pub fn get_escrow_ids_by_status(
        env: Env,
        status: EscrowStatus,
        offset: u32,
        limit: u32,
    ) -> Vec<u64> {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));
        let mut results = Vec::new(&env);
        let mut count = 0u32;
        let mut skipped = 0u32;

        for i in 0..index.len() {
            if count >= limit {
                break;
            }
            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                if escrow.status == status {
                    if skipped < offset {
                        skipped += 1;
                        continue;
                    }
                    results.push_back(bounty_id);
                    count += 1;
                }
            }
        }
        results
    }

    pub fn set_anti_abuse_admin(env: Env, admin: Address) -> Result<(), Error> {
        let current: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        current.require_auth();
        anti_abuse::set_admin(&env, admin);
        Ok(())
    }

    pub fn get_anti_abuse_admin(env: Env) -> Option<Address> {
        anti_abuse::get_admin(&env)
    }

    pub fn set_whitelist(
        env: Env,
        whitelisted_address: Address,
        whitelisted: bool,
    ) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        anti_abuse::set_whitelist(&env, whitelisted_address, whitelisted);
        Ok(())
    }

    /// Retrieves the refund history for a specific bounty.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `bounty_id` - The bounty to query
    ///
    /// # Returns
    /// * `Ok(Vec<RefundRecord>)` - The refund history
    /// * `Err(Error::BountyNotFound)` - Bounty doesn't exist
    pub fn get_refund_history(env: Env, bounty_id: u64) -> Result<Vec<RefundRecord>, Error> {
        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();
        Ok(escrow.refund_history)
    }

    // ========================================================================
    // Governance Integration
    // ========================================================================

    /// Set the governance contract address (admin only)
    pub fn set_governance_contract(env: Env, governance_addr: Address) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        governance_integration::set_governance_contract(&env, governance_addr);
        Ok(())
    }

    /// Get the governance contract address
    pub fn get_governance_contract(env: Env) -> Option<Address> {
        governance_integration::get_governance_contract(&env)
    }

    /// Set minimum required governance version (admin only)
    pub fn set_min_governance_version(env: Env, min_version: u32) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        governance_integration::set_min_governance_version(&env, min_version);
        Ok(())
    }

    /// Get minimum required governance version
    pub fn get_min_governance_version(env: Env) -> u32 {
        governance_integration::get_min_governance_version(&env)
    }

    /// Check if governance requirements are met before admin operations
    fn check_governance_requirements(env: &Env) -> Result<(), Error> {
        if !governance_integration::check_governance_version(env) {
            return Err(Error::GovernanceVersionTooLow);
        }
        Ok(())
    }

    /// Gets refund eligibility information for a bounty.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `bounty_id` - The bounty to query
    ///
    /// # Returns
    /// * `Ok((bool, bool, i128, Option<RefundApproval>))` - Tuple containing:
    ///   - can_refund: Whether refund is possible
    ///   - deadline_passed: Whether the deadline has passed
    ///   - remaining: Remaining amount in escrow
    ///   - approval: Optional refund approval if exists
    /// * `Err(Error::BountyNotFound)` - Bounty doesn't exist
    pub fn get_refund_eligibility(
        env: Env,
        bounty_id: u64,
    ) -> Result<(bool, bool, i128, Option<RefundApproval>), Error> {
        if !env.storage().persistent().has(&DataKey::Escrow(bounty_id)) {
            return Err(Error::BountyNotFound);
        }
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(bounty_id))
            .unwrap();

        let now = env.ledger().timestamp();
        let deadline_passed = now >= escrow.deadline;

        let approval = if env
            .storage()
            .persistent()
            .has(&DataKey::RefundApproval(bounty_id))
        {
            Some(
                env.storage()
                    .persistent()
                    .get(&DataKey::RefundApproval(bounty_id))
                    .unwrap(),
            )
        } else {
            None
        };

        // can_refund is true if:
        // 1. Status is Locked or PartiallyRefunded AND
        // 2. (deadline has passed OR there's an approval)
        let can_refund = (escrow.status == EscrowStatus::Locked
            || escrow.status == EscrowStatus::PartiallyRefunded)
            && (deadline_passed || approval.is_some());

        Ok((
            can_refund,
            deadline_passed,
            escrow.remaining_amount,
            approval,
        ))
    }

    /// Batch lock funds for multiple bounties in a single transaction.
    /// This improves gas efficiency by reducing transaction overhead.
    ///
    /// # Arguments
    /// * `items` - Vector of LockFundsItem containing bounty_id, depositor, amount, and deadline
    ///
    /// # Returns
    /// Number of successfully locked bounties
    ///
    /// # Errors
    /// * InvalidBatchSize - if batch size exceeds MAX_BATCH_SIZE or is zero
    /// * BountyExists - if any bounty_id already exists
    /// * NotInitialized - if contract is not initialized
    ///
    /// # Note
    /// This operation is atomic - if any item fails, the entire transaction reverts.
    pub fn batch_lock_funds(env: Env, items: Vec<LockFundsItem>) -> Result<u32, Error> {
        if Self::check_paused(&env, symbol_short!("lock")) {
            return Err(Error::FundsPaused);
        }

        // Check circuit breaker before proceeding
        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }
        // Validate batch size
        let batch_size = items.len() as u32;
        if batch_size == 0 {
            return Err(Error::InvalidBatchSize);
        }
        if batch_size > MAX_BATCH_SIZE {
            return Err(Error::InvalidBatchSize);
        }

        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);
        let contract_address = env.current_contract_address();
        let timestamp = env.ledger().timestamp();

        // Validate all items before processing (all-or-nothing approach)
        for item in items.iter() {
            // Check if bounty already exists
            if env
                .storage()
                .persistent()
                .has(&DataKey::Escrow(item.bounty_id))
            {
                return Err(Error::BountyExists);
            }

            // Validate amount
            if item.amount <= 0 {
                return Err(Error::InvalidAmount);
            }

            // Check for duplicate bounty_ids in the batch
            let mut count = 0u32;
            for other_item in items.iter() {
                if other_item.bounty_id == item.bounty_id {
                    count += 1;
                }
            }
            if count > 1 {
                return Err(Error::DuplicateBountyId);
            }
        }

        // Collect unique depositors and require auth once for each
        // This prevents "frame is already authorized" errors when same depositor appears multiple times
        let mut seen_depositors: Vec<Address> = Vec::new(&env);
        for item in items.iter() {
            let mut found = false;
            for seen in seen_depositors.iter() {
                if seen.clone() == item.depositor {
                    found = true;
                    break;
                }
            }
            if !found {
                seen_depositors.push_back(item.depositor.clone());
                item.depositor.require_auth();
            }
        }

        // Process all items (atomic - all succeed or all fail)
        let mut locked_count = 0u32;
        for item in items.iter() {
            // Transfer funds from depositor to contract
            client.transfer(&item.depositor, &contract_address, &item.amount);

            // Create escrow record
            let escrow = Escrow {
                depositor: item.depositor.clone(),
                amount: item.amount,
                status: EscrowStatus::Locked,
                deadline: item.deadline,
                refund_history: vec![&env],
                remaining_amount: item.amount,
            };

            // Store escrow
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(item.bounty_id), &escrow);
            Self::bump_escrow_ttl(&env, item.bounty_id);

            let mut index: Vec<u64> = env
                .storage()
                .persistent()
                .get(&DataKey::EscrowIndex)
                .unwrap_or(Vec::new(&env));
            index.push_back(item.bounty_id);
            env.storage()
                .persistent()
                .set(&DataKey::EscrowIndex, &index);

            let depositor_key = DataKey::DepositorIndex(item.depositor.clone());
            let mut depositor_index: Vec<u64> = env
                .storage()
                .persistent()
                .get(&depositor_key)
                .unwrap_or(Vec::new(&env));
            depositor_index.push_back(item.bounty_id);
            env.storage().persistent().set(&depositor_key, &depositor_index);
            Self::bump_escrow_index_ttl(&env, item.depositor.clone());

            // Initialize analytics for this bounty
            init_bounty_analytics(&env, item.bounty_id, item.amount, timestamp);

            // Update incremental aggregate counters
            Self::increment_locked(&env, item.amount);

            // Emit individual event for each locked bounty
            emit_funds_locked(
                &env,
                FundsLocked {
                    version: EVENT_VERSION_V2,
                    bounty_id: item.bounty_id,
                    amount: item.amount,
                    depositor: item.depositor.clone(),
                    deadline: item.deadline,
                },
            );

            locked_count += 1;
        }

        // Emit batch event
        emit_batch_funds_locked(
            &env,
            BatchFundsLocked {
                version: EVENT_VERSION_V2,
                count: locked_count,
                total_amount: items.iter().map(|i| i.amount).sum(),
                timestamp,
            },
        );

        Ok(locked_count)
    }

    /// Batch release funds to multiple contributors in a single transaction.
    /// This improves gas efficiency by reducing transaction overhead.
    ///
    /// # Arguments
    /// * `items` - Vector of ReleaseFundsItem containing bounty_id and contributor address
    ///
    /// # Returns
    /// Number of successfully released bounties
    ///
    /// # Errors
    /// * InvalidBatchSize - if batch size exceeds MAX_BATCH_SIZE or is zero
    /// * BountyNotFound - if any bounty_id doesn't exist
    /// * FundsNotLocked - if any bounty is not in Locked status
    /// * Unauthorized - if caller is not admin
    ///
    /// # Note
    /// This operation is atomic - if any item fails, the entire transaction reverts.
    /// If governance is configured, the linked governance version must meet
    /// the configured minimum before any item is released.
    pub fn batch_release_funds(env: Env, items: Vec<ReleaseFundsItem>) -> Result<u32, Error> {
        if Self::check_paused(&env, symbol_short!("release")) {
            return Err(Error::FundsPaused);
        }

        // Check circuit breaker before proceeding
        if let Err(_) = check_and_allow(&env) {
            return Err(Error::CircuitBreakerOpen);
        }

        Self::check_governance_requirements(&env)?;

        // Validate batch size
        let batch_size = items.len() as u32;
        if batch_size == 0 {
            return Err(Error::InvalidBatchSize);
        }
        if batch_size > MAX_BATCH_SIZE {
            return Err(Error::InvalidBatchSize);
        }

        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);
        let contract_address = env.current_contract_address();
        let timestamp = env.ledger().timestamp();

        // Validate all items before processing (all-or-nothing approach)
        let mut total_amount: i128 = 0;
        for item in items.iter() {
            // Check if bounty exists
            if !env
                .storage()
                .persistent()
                .has(&DataKey::Escrow(item.bounty_id))
            {
                return Err(Error::BountyNotFound);
            }

            let escrow: Escrow = env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(item.bounty_id))
                .unwrap();

            // Check if funds are locked
            if escrow.status != EscrowStatus::Locked {
                return Err(Error::FundsNotLocked);
            }

            // Dispute protection: batch release must also respect open disputes.
            if env
                .storage()
                .persistent()
                .has(&DataKey::PendingClaim(item.bounty_id))
            {
                return Err(Error::RefundNotApproved);
            }

            // Check for duplicate bounty_ids in the batch
            let mut count = 0u32;
            for other_item in items.iter() {
                if other_item.bounty_id == item.bounty_id {
                    count += 1;
                }
            }
            if count > 1 {
                return Err(Error::DuplicateBountyId);
            }

            total_amount = total_amount
                .checked_add(escrow.amount)
                .ok_or(Error::InvalidAmount)?;
        }

        // Process all items (atomic - all succeed or all fail)
        let mut released_count = 0u32;
        for item in items.iter() {
            let mut escrow: Escrow = env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(item.bounty_id))
                .unwrap();
            Self::bump_escrow_ttl(&env, item.bounty_id);

            // Transfer funds to contributor
            client.transfer(&contract_address, &item.contributor, &escrow.amount);

            // Update escrow status; zero remaining_amount to maintain invariant.
            escrow.status = EscrowStatus::Released;
            escrow.remaining_amount = 0;
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(item.bounty_id), &escrow);
            Self::bump_escrow_ttl(&env, item.bounty_id);

            // Update incremental aggregate counters
            Self::transition_locked_to_released(&env, escrow.amount);

            // Emit individual event for each released bounty
            emit_funds_released(
                &env,
                FundsReleased {
                    version: EVENT_VERSION_V2,
                    bounty_id: item.bounty_id,
                    amount: escrow.amount,
                    recipient: item.contributor.clone(),
                    timestamp,
                },
            );

            released_count += 1;
        }

        // Emit batch event
        emit_batch_funds_released(
            &env,
            BatchFundsReleased {
                version: EVENT_VERSION_V2,
                count: released_count,
                total_amount,
                timestamp,
            },
        );

        Ok(released_count)
    }

    // ==================== ANALYTICS VIEW FUNCTIONS ====================

    /// Get per-bounty analytics for a specific bounty
    ///
    /// # Arguments
    /// * `bounty_id` - The bounty to query
    ///
    /// # Returns
    /// * `Ok(BountyAnalytics)` - Analytics for the bounty including amounts locked/released/refunded
    /// * `Err(Error::BountyNotFound)` - If bounty doesn't exist
    pub fn get_bounty_analytics(env: Env, bounty_id: u64) -> Result<analytics::BountyAnalytics, Error> {
        get_bounty_analytics(&env, bounty_id).ok_or(Error::BountyNotFound)
    }

    /// Get contract-wide analytics snapshot
    ///
    /// Returns aggregated metrics about active bounties, total locked amounts, and released amounts.
    /// This view is efficient and suitable for regular polling by off-chain indexers.
    ///
    /// # Returns
    /// `ContractAnalytics` containing:
    /// - Active bounty count (Locked or Partially Refunded)
    /// - Released bounty count
    /// - Refunded bounty count
    /// - Total locked amount
    /// - Total released amount
    /// - Total refunded amount
    /// - Average bounty size
    ///
    /// This function uses O(1) incremental counters for efficiency.
    pub fn get_contract_analytics(env: Env) -> ContractAnalytics {
        let stats = Self::get_counters(&env);

        let total_count = (stats.count_locked as i128)
            .saturating_add(stats.count_released as i128)
            .saturating_add(stats.count_refunded as i128);
        let average_bounty = if total_count > 0 {
            (stats.total_locked
                .saturating_add(stats.total_released)
                .saturating_add(stats.total_refunded))
                / total_count
        } else {
            0
        };

        ContractAnalytics {
            active_bounty_count: stats.count_locked,
            released_bounty_count: stats.count_released,
            refunded_bounty_count: stats.count_refunded,
            total_locked: stats.total_locked,
            total_released: stats.total_released,
            total_refunded: stats.total_refunded,
            average_bounty_amount: average_bounty,
            snapshot_timestamp: env.ledger().timestamp(),
        }
    }

    /// Emit a contract analytics snapshot event
    ///
    /// This can be called periodically to create snapshots for off-chain analytics and indexing.
    /// The event contains a full `ContractAnalytics` structure that can be ingested by indexing services.
    pub fn emit_analytics_snapshot_event(env: Env) {
        let analytics = Self::get_contract_analytics(env.clone());
        emit_analytics_snapshot(
            &env,
            AnalyticsSnapshot {
                version: analytics::ANALYTICS_VERSION_V1,
                metrics: analytics,
            },
        );
    }

    /// Count bounties by status
    ///
    /// # Returns
    /// Number of bounties in the specified status
    /// Count bounties by status using O(1) incremental counters.
    ///
    /// For Locked status, this includes both Locked and PartiallyRefunded bounties.
    /// For other statuses, only exact matches are counted.
    pub fn count_bounties_by_status(env: Env, status: EscrowStatus) -> u32 {
        let stats = Self::get_counters(&env);
        match status {
            EscrowStatus::Locked => stats.count_locked,
            EscrowStatus::Released => stats.count_released,
            EscrowStatus::Refunded => stats.count_refunded,
            EscrowStatus::PartiallyRefunded => {
                // PartiallyRefunded is included in count_locked
                // To get exact PartiallyRefunded count, need full scan
                Self::count_by_status_full_scan(env, status)
            }
        }
    }

    /// Count bounties by status via full O(N) scan for exact status matching.
    ///
    /// Use this when you need to distinguish between Locked and PartiallyRefunded,
    /// or for verification purposes.
    pub fn count_by_status_full_scan(env: Env, status: EscrowStatus) -> u32 {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));

        let mut count = 0u32;
        for i in 0..index.len() {
            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                if escrow.status == status {
                    count += 1;
                }
            }
        }
        count
    }

    /// Get total volume of funds by status using O(1) incremental counters.
    ///
    /// Returns the sum of all funds in bounties with the specified status.
    pub fn get_volume_by_status(env: Env, status: EscrowStatus) -> i128 {
        let stats = Self::get_counters(&env);
        match status {
            EscrowStatus::Locked => stats.total_locked,
            EscrowStatus::Released => stats.total_released,
            EscrowStatus::Refunded => stats.total_refunded,
            EscrowStatus::PartiallyRefunded => {
                // PartiallyRefunded is included in total_locked
                // To get exact PartiallyRefunded volume, need full scan
                Self::volume_by_status_full_scan(env, status)
            }
        }
    }

    /// Get total volume of funds by status via full O(N) scan.
    ///
    /// Use this for exact status matching or verification.
    pub fn volume_by_status_full_scan(env: Env, status: EscrowStatus) -> i128 {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));

        let mut total = 0i128;
        for i in 0..index.len() {
            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                if escrow.status == status {
                    let amount = match status {
                        EscrowStatus::Locked | EscrowStatus::PartiallyRefunded => {
                            escrow.remaining_amount
                        }
                        _ => escrow.amount,
                    };
                    total = total.saturating_add(amount);
                }
            }
        }
        total
    }

    /// Get statistics for a specific depositor
    ///
    /// Returns count and total amount of bounties created by the depositor
    pub fn get_depositor_stats(
        env: Env,
        depositor: Address,
    ) -> (u32, i128, u32, i128, u32, i128) {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::DepositorIndex(depositor.clone()))
            .unwrap_or(Vec::new(&env));

        let mut locked_count = 0u32;
        let mut locked_amount = 0i128;
        let mut released_count = 0u32;
        let mut released_amount = 0i128;
        let mut refunded_count = 0u32;
        let mut refunded_amount = 0i128;

        for i in 0..index.len() {
            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                match escrow.status {
                    EscrowStatus::Locked | EscrowStatus::PartiallyRefunded => {
                        locked_count += 1;
                        locked_amount = locked_amount.saturating_add(escrow.remaining_amount);
                    }
                    EscrowStatus::Released => {
                        released_count += 1;
                        released_amount = released_amount.saturating_add(escrow.amount);
                    }
                    EscrowStatus::Refunded => {
                        refunded_count += 1;
                        refunded_amount = refunded_amount.saturating_add(escrow.amount);
                    }
                }
            }
        }

        (
            locked_count,
            locked_amount,
            released_count,
            released_amount,
            refunded_count,
            refunded_amount,
        )
    }

    /// Query bounties by expiration status (approaching or already expired)
    ///
    /// # Arguments
    /// * `max_deadline` - Only return bounties with deadline <= this timestamp
    /// * `offset` - Pagination offset
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    /// Vector of bounties sorted by deadline that match the criteria
    pub fn query_expiring_bounties(env: Env, max_deadline: u64, offset: u32, limit: u32) -> Vec<u64> {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));

        let mut results = Vec::new(&env);
        let mut count = 0u32;
        let mut skipped = 0u32;

        for i in 0..index.len() {
            if count >= limit {
                break;
            }

            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                // Only include locked/partially refunded bounties that are expiring
                if (escrow.status == EscrowStatus::Locked
                    || escrow.status == EscrowStatus::PartiallyRefunded)
                    && escrow.deadline <= max_deadline
                {
                    if skipped < offset {
                        skipped += 1;
                        continue;
                    }
                    results.push_back(bounty_id);
                    count += 1;
                }
            }
        }

        results
    }

    /// Get high-value bounties (above a threshold) for risk monitoring.
    ///
    /// **Note:** This function still performs an O(N) scan as it requires filtering
    /// by amount, which cannot be efficiently maintained in aggregate counters.
    /// Consider using pagination and caching for large datasets.
    ///
    /// # Arguments
    /// * `min_amount` - Minimum amount to consider "high-value"
    /// * `limit` - Maximum number of results
    pub fn get_high_value_bounties(env: Env, min_amount: i128, limit: u32) -> Vec<u64> {
        let index: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowIndex)
            .unwrap_or(Vec::new(&env));

        let mut results = Vec::new(&env);
        let mut count = 0u32;

        for i in 0..index.len() {
            if count >= limit {
                break;
            }

            let bounty_id = index.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(bounty_id))
            {
                if escrow.amount >= min_amount {
                    results.push_back(bounty_id);
                    count += 1;
                }
            }
        }

        results
    }

    // ==================== CIRCUIT BREAKER ADMIN CONTROLS ====================

    /// Set the circuit breaker admin address (admin only).
    /// The circuit breaker admin can reset the circuit when it opens.
    pub fn set_circuit_breaker_admin(
        env: Env,
        admin: Address,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let current_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        current_admin.require_auth();

        set_circuit_admin(&env, admin, Some(current_admin));
        Ok(())
    }

    /// Get the circuit breaker admin address, if set.
    pub fn get_circuit_breaker_admin(env: Env) -> Option<Address> {
        get_circuit_admin(&env)
    }

    /// Configure the circuit breaker thresholds (admin only).
    pub fn set_circuit_breaker_config(
        env: Env,
        failure_threshold: u32,
        success_threshold: u32,
        max_error_log: u32,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        set_config(
            &env,
            CircuitBreakerConfig {
                failure_threshold,
                success_threshold,
                max_error_log,
            },
        );
        Ok(())
    }

    /// Get the current circuit breaker configuration.
    pub fn get_circuit_breaker_config(env: Env) -> CircuitBreakerConfig {
        get_config(&env)
    }

    /// Get the current circuit breaker status.
    pub fn get_circuit_breaker_status(env: Env) -> CircuitBreakerStatus {
        get_status(&env)
    }

    /// Reset the circuit breaker (circuit breaker admin only).
    /// Transitions: Open -> HalfOpen, or HalfOpen/Closed -> Closed.
    pub fn reset_circuit(
        env: Env,
        admin: Address,
    ) -> Result<(), Error> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::NotInitialized);
        }

        // The reset_circuit_breaker function handles auth internally
        reset_circuit_breaker(&env, &admin);
        Ok(())
    }

    /// Get the circuit breaker error log.
    pub fn get_circuit_error_log(env: Env) -> soroban_sdk::Vec<ErrorEntry> {
        error_recovery::get_error_log(&env)
    }

    // ==================== END CIRCUIT BREAKER ADMIN CONTROLS ====================
}


#[cfg(test)]
mod test;
#[cfg(test)]
mod test_analytics_monitoring;
#[cfg(test)]
mod test_auto_refund_permissions;
#[cfg(test)]
mod test_bounty_escrow;
#[cfg(test)]
mod test_dispute_resolution;
mod test_expiration_and_dispute;
#[cfg(test)]
mod test_granular_pause;
#[cfg(test)]
mod test_lifecycle;
#[cfg(test)]
mod test_pause;
#[cfg(test)]
mod proptest_invariants;
#[cfg(test)]
mod test_query_filters;
#[cfg(test)]
mod test_governance_integration;
#[cfg(test)]
mod test_circuit_breaker;
mod test_bounty_analytics;
#[cfg(test)]
mod test_gas_proxy;

#[cfg(test)]
mod test_reentrancy;

#[cfg(test)]
mod test_balance_invariant;
#[cfg(test)]
mod test_upgrade_scenarios;
