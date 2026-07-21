//! # grainlify-core
//!
//! The crate root for the Grainlify upgrade-governance system on Soroban/Stellar.
//! It wires together two **independent, parallel** upgrade primitives —
//! [`governance`] (democratic on-chain voting) and [`multisig`] (threshold
//! key-holder approval) — and exposes them through a single deployable contract:
//! [`GrainlifyContract`].
//!
//! Neither module depends on the other. A deployment may use one, both, or
//! neither upgrade path; the choice is made at runtime by calling the
//! appropriate initialiser and entrypoints.
//!
//! ## Module Relationships
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │                        grainlify-core                                │
//! │                      (GrainlifyContract)                             │
//! │                                                                      │
//! │   ┌──────────────────────────┐   ┌───────────────────────────────┐  │
//! │   │     mod governance       │   │       mod multisig            │  │
//! │   │  (GovernanceContract)    │   │        (MultiSig)             │  │
//! │   │                          │   │                               │  │
//! │   │  Democratic on-chain     │   │  Threshold key-holder         │  │
//! │   │  voting for upgrade      │   │  approval for WASM upgrades.  │  │
//! │   │  proposals.              │   │                               │  │
//! │   │                          │   │  N-of-M signers must approve  │  │
//! │   │  Any eligible voter may  │   │  before execute_upgrade()     │  │
//! │   │  participate; quorum +   │   │  can run.  Replay-protected   │  │
//! │   │  threshold decide the    │   │  via a monotonic nonce.       │  │
//! │   │  outcome.                │   │                               │  │
//! │   │                          │   │  Signer set and threshold     │  │
//! │   │  is_upgrade_approved()   │   │  are snapshotted per-         │  │
//! │   │  records approved WASM   │   │  proposal to prevent          │  │
//! │   │  hashes for cross-       │   │  retroactive config changes.  │  │
//! │   │  contract verification.  │   │                               │  │
//! │   └──────────────────────────┘   └───────────────────────────────┘  │
//! │            ▲                                                         │
//! │            │ cross-contract call                                     │
//! │   ┌────────┴──────────────────────────────────────────────────────┐  │
//! │   │          bounty_escrow  /  program-escrow                     │  │
//! │   │          governance_integration.rs                            │  │
//! │   │                                                               │  │
//! │   │  GovernanceInterface (contractclient trait):                  │  │
//! │   │    get_ver()                   → version liveness check       │  │
//! │   │    is_upg_ok(wasm_hash)        → upgrade approval gate        │  │
//! │   │    get_version_numeric_encoded() → semver gate (bounty only)  │  │
//! │   └───────────────────────────────────────────────────────────────┘  │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## governance module
//!
//! `governance.rs` implements [`GovernanceContract`], a standalone democratic
//! voting system for on-chain upgrade proposals.
//!
//! ### Public interface
//!
//! | Entrypoint | Description |
//! |---|---|
//! | `init_governance(admin, config)` | One-time governance initialisation. Stores [`GovernanceConfig`] and sets proposal counter to 0. |
//! | `create_proposal(proposer, wasm_hash, description)` | Opens a new upgrade proposal for voting. Enforces `min_proposal_stake` if configured. |
//! | `cast_vote(voter, proposal_id, vote_type)` | Casts a `For`, `Against`, or `Abstain` vote. Derives voting power from the configured scheme. |
//! | `finalize_proposal(proposal_id)` | After the voting period, evaluates quorum + threshold and sets the proposal to `Approved` or `Rejected`. |
//! | `execute_proposal(proposal_id)` | Marks an `Approved` proposal as `Executed` after its execution delay. Does **not** perform a WASM update — it records that governance blessed the hash. |
//! | `is_upgrade_approved(wasm_hash)` | Returns `true` if an `Executed` proposal for this hash exists and its execution delay has elapsed. Used by escrow contracts as an upgrade gate. |
//! | `get_proposal_status(proposal_id)` | Read-only status query. |
//! | `sweep_expired_proposal(proposal_id, current_time)` | Flags an unfinalized, past-deadline proposal as `Expired` (flag-not-delete pattern). |
//!
//! ### Voting schemes
//!
//! [`VotingScheme::OnePersonOneVote`] assigns power `1` to every authenticated
//! voter.  [`VotingScheme::TokenWeighted`] reads each voter's current balance
//! from the configured governance token; see `GOVERNANCE.md` for snapshot
//! caveats.
//!
//! ### Exported types
//!
//! [`GovernanceConfig`], [`Proposal`], [`ProposalStatus`], [`Vote`],
//! [`VoteType`], [`VotingScheme`], and [`Error as GovError`] are all
//! re-exported from the crate root for use by external consumers.
//!
//! ## multisig module
//!
//! `multisig.rs` implements [`MultiSig`], a pure threshold-approval primitive
//! for WASM upgrade proposals.  It is **entirely independent of the governance
//! module**: it has no knowledge of voting periods, quorum, or governance
//! config.
//!
//! ### Public interface
//!
//! | Method | Description |
//! |---|---|
//! | `MultiSig::init(env, signers, threshold)` | One-time initialisation. Stores the signer set and threshold; rejects `threshold == 0` or `threshold > signers.len()`. |
//! | `MultiSig::propose(env, proposer, action)` | Creates an `Upgrade(wasm_hash)` proposal, snapshotting the current signer set and threshold into the proposal record. |
//! | `MultiSig::approve(env, proposal_id, signer)` | Records one signer's approval. Rejects non-signers, duplicate approvals, and already-executed proposals. |
//! | `MultiSig::can_execute(env, proposal_id)` | Returns `true` when the proposal's approval count meets its **snapshotted** threshold. |
//! | `MultiSig::nonce(env)` | Returns the next expected execution nonce (replay protection). |
//! | `MultiSig::execute(env, proposal_id, expected_action, expected_nonce, closure)` | Atomically verifies threshold, payload, and nonce, runs the provided closure (the WASM update), marks the proposal executed, and increments the nonce. |
//! | `MultiSig::get_action(env, proposal_id)` | Returns the `ProposalAction` bound to a proposal. |
//! | `MultiSig::remove_signer(env, caller, signer_to_remove)` | Removes a signer; rejected if `(signer_count - 1) < threshold` to prevent permanent lockout. |
//!
//! ### Key properties
//!
//! - **Payload binding**: signers approve a specific `wasm_hash`; executing a
//!   different hash panics with `ActionMismatch`.
//! - **Snapshot isolation**: changing the global signer config after a proposal
//!   is created cannot retroactively satisfy or invalidate that proposal.
//! - **Replay protection**: `execute_upgrade` requires the current nonce; it is
//!   consumed on success so the same `(approvals, nonce)` pair cannot be
//!   replayed.
//!
//! ## Single-admin upgrade path
//!
//! In addition to the two decentralised upgrade paths, `GrainlifyContract`
//! provides a **single-admin** upgrade route (`init_admin` / `schedule_upgrade`
//! / `upgrade`). This path does **not** use `governance` or `multisig`; it
//! requires one trusted admin address and a mandatory schedule/timelock before
//! `upgrade` can execute.
//!
//! The three upgrade paths are mutually exclusive at initialisation:
//! - `init(signers, threshold)` — activates the multisig path.
//! - `init_governance(admin, config)` — activates the governance path.
//! - `init_admin(admin)` — activates the single-admin + timelock path.
//!
//! ## Consumption by bounty_escrow and program-escrow
//!
//! Both escrow contracts integrate with `grainlify-core` **exclusively through
//! the governance side**.  Neither escrow contract calls any multisig
//! entrypoint.
//!
//! Each escrow crate ships a `governance_integration.rs` module that declares
//! the following cross-contract interface:
//!
//! ```rust,ignore
//! #[contractclient(name = "GovernanceClient")]
//! trait GovernanceInterface {
//!     fn get_ver(env: Env) -> u32;
//!     fn is_upg_ok(env: Env, wasm_hash: BytesN<32>) -> bool;
//!     // bounty_escrow also calls:
//!     fn get_version_numeric_encoded(env: Env) -> u32;
//! }
//! ```
//!
//! | Method called | Provided by | Purpose |
//! |---|---|---|
//! | `get_ver()` | `GrainlifyContract::get_ver` | Version liveness check; ensures the governance contract is reachable and meets a minimum version before an upgrade gate is evaluated. |
//! | `is_upg_ok(wasm_hash)` | `GrainlifyContract::is_upg_ok` → `GovernanceContract::is_upgrade_approved` | Returns `true` only when an executed, post-delay governance proposal for `wasm_hash` exists. Escrow contracts call this before permitting a WASM upgrade. |
//! | `get_version_numeric_encoded()` | `GrainlifyContract::get_version_numeric_encoded` | Used by `bounty_escrow` to enforce a minimum semver gate (`major*10_000 + minor*100 + patch`). |
//!
//! ### Integration contract: architecture boundary
//!
//! ```text
//! bounty_escrow / program-escrow
//!   └── governance_integration::check_upgrade_approval(env, wasm_hash)
//!           │
//!           │  cross-contract call via GovernanceClient
//!           ▼
//!       GrainlifyContract::is_upg_ok(wasm_hash)
//!           │
//!           ▼
//!       GovernanceContract::is_upgrade_approved(wasm_hash)
//!           └── scans proposals for Executed status + matching hash
//! ```
//!
//! The escrow contracts do **not** need to know whether `grainlify-core` was
//! configured with governance, multisig, or single-admin.  They only ask "has
//! governance approved this hash?" via `is_upg_ok`.  The `multisig` and
//! single-admin paths are entirely internal to `grainlify-core`.
//!
//! ## Monitoring module (inline)
//!
//! `lib.rs` contains an inline `monitoring` module with persistent counters for
//! observability.  All counters are **observational only**; no entrypoint reads
//! them to gate authorization or control flow.
//!
//! | Counter | Storage key | Incremented by |
//! |---|---|---|
//! | `proposals_created` | `gov_prop` | `create_proposal` (on success) |
//! | `votes_cast` | `gov_vote` | `cast_vote` (on success) |
//! | `upgrades_executed` | `gov_upg` | `upgrade` and `execute_upgrade` (after WASM update) |
//! | `migrations_run` | `gov_migr` | `migrate` (once per applied migration) |
//!
//! Counters are surfaced through `get_analytics()` and `get_state_snapshot()`,
//! and each increment emits a `GovernanceMetric` event under topic
//! `("metric", "gov")` for indexer consumption.
//!
//! ## Public re-exports
//!
//! The following types from `governance` are re-exported at the crate root:
//!
//! - [`GovError`] (`governance::Error`)
//! - [`GovernanceConfig`]
//! - [`Proposal`] (governance proposal, not multisig proposal)
//! - [`ProposalStatus`]
//! - [`Vote`]
//! - [`VoteType`]
//! - [`VotingScheme`]
//!
//! `multisig` types (`MultiSig`, `ProposalAction`) are used internally and are
//! **not** re-exported.
//!
//! ## Security notes
//!
//! - Governance and multisig are **parallel, non-dependent** upgrade paths.
//!   Multisig is not a prerequisite for governance execution, and governance
//!   approval is not required by the multisig path.
//! - The single-admin path enforces a configurable timelock (default 24 h,
//!   minimum 5 min) between `schedule_upgrade` and `upgrade`.
//! - All three paths emit a versioned `UpgradeExecuted` event (`upg_exec2`)
//!   that indexers can use to track WASM changes regardless of which path was
//!   used.
//! - Accurate architecture docs here reduce the risk of a future integration
//!   mistake at the governance/multisig boundary (e.g., assuming multisig
//!   approval is required before calling `is_upg_ok`).
//!
//! For in-depth governance flow, voting-scheme caveats, event schemas, and
//! timelock details, see `docs/grainlify-core/GOVERNANCE.md`.

#![no_std]

mod governance;
mod multisig;
pub use governance::{
    Error as GovError, GovernanceConfig, Proposal, ProposalStatus, Vote, VoteType, VotingScheme,
};
use multisig::{MultiSig, ProposalAction};
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, BytesN, Env, String, Symbol, Vec,
};

// ==================== MONITORING MODULE ====================
mod monitoring {
    use soroban_sdk::{contracttype, symbol_short, Address, Env, String, Symbol};

    // Storage keys
    const OPERATION_COUNT: &str = "op_count";
    const USER_COUNT: &str = "usr_count";
    const ERROR_COUNT: &str = "err_count";

    // Governance / upgrade metric counter keys.
    //
    // These persistent counters track security-critical governance and upgrade
    // activity so operators can observe how many proposals, votes, upgrades, and
    // migrations the contract has processed over its lifetime.
    const PROPOSALS_CREATED: &str = "gov_prop";
    const VOTES_CAST: &str = "gov_vote";
    const UPGRADES_EXECUTED: &str = "gov_upg";
    const MIGRATIONS_RUN: &str = "gov_migr";

    // Event: Operation metric
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct OperationMetric {
        pub operation: Symbol,
        pub caller: Address,
        pub timestamp: u64,
        pub success: bool,
    }

    // Event: Governance metric
    //
    // Emitted whenever a governance/upgrade counter is incremented. Mirrors the
    // escrow contracts' metric event pattern so indexers can consume a single,
    // consistent metric stream across contracts.
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct GovernanceMetric {
        pub metric: Symbol,
        pub total: u64,
        pub timestamp: u64,
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
        // Real governance / upgrade activity counters.
        pub proposals_created: u64,
        pub votes_cast: u64,
        pub upgrades_executed: u64,
        pub migrations_run: u64,
    }

    // Data: State snapshot
    #[contracttype]
    #[derive(Clone, Debug)]
    pub struct StateSnapshot {
        pub timestamp: u64,
        pub total_operations: u64,
        pub total_users: u64,
        pub total_errors: u64,
        // Real governance / upgrade activity counters.
        pub proposals_created: u64,
        pub votes_cast: u64,
        pub upgrades_executed: u64,
        pub migrations_run: u64,
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

    /// Increments the persistent counter stored under `key` and returns the new total.
    ///
    /// The counter is saturating, so it can never wrap or panic. Counters written
    /// here are observational only: no governance or upgrade entrypoint ever reads
    /// them to gate authorization or alter control flow.
    fn increment_counter(env: &Env, key: &str) -> u64 {
        let storage_key = Symbol::new(env, key);
        let current: u64 = env.storage().persistent().get(&storage_key).unwrap_or(0);
        let updated = current.saturating_add(1);
        env.storage().persistent().set(&storage_key, &updated);
        updated
    }

    /// Reads the persistent counter stored under `key`, defaulting to `0`.
    ///
    /// This is a pure read used by the analytics/state-snapshot views. It never
    /// mutates storage or influences contract control flow.
    fn read_counter(env: &Env, key: &str) -> u64 {
        env.storage()
            .persistent()
            .get(&Symbol::new(env, key))
            .unwrap_or(0)
    }

    /// Emits a governance metric event mirroring the escrow contracts' pattern.
    fn emit_governance_metric(env: &Env, metric: Symbol, total: u64) {
        env.events().publish(
            (symbol_short!("metric"), symbol_short!("gov")),
            GovernanceMetric {
                metric,
                total,
                timestamp: env.ledger().timestamp(),
            },
        );
    }

    /// Records that a governance proposal was created and returns the running total.
    ///
    /// Call after the proposal has been persisted successfully so the counter only
    /// reflects real proposals. Purely observational; never affects auth.
    pub fn track_proposal_created(env: &Env) -> u64 {
        let total = increment_counter(env, PROPOSALS_CREATED);
        emit_governance_metric(env, symbol_short!("proposal"), total);
        total
    }

    /// Records that a governance vote was cast and returns the running total.
    ///
    /// Call after a vote has been persisted successfully. Purely observational.
    pub fn track_vote_cast(env: &Env) -> u64 {
        let total = increment_counter(env, VOTES_CAST);
        emit_governance_metric(env, symbol_short!("vote"), total);
        total
    }

    /// Records that a contract upgrade was executed and returns the running total.
    ///
    /// Call after the WASM update has been applied. Purely observational.
    pub fn track_upgrade_executed(env: &Env) -> u64 {
        let total = increment_counter(env, UPGRADES_EXECUTED);
        emit_governance_metric(env, symbol_short!("upgrade"), total);
        total
    }

    /// Records that a state migration ran and returns the running total.
    ///
    /// Call once a migration has completed so idempotent re-invocations that exit
    /// early are not double-counted. Purely observational.
    pub fn track_migration_run(env: &Env) -> u64 {
        let total = increment_counter(env, MIGRATIONS_RUN);
        emit_governance_metric(env, symbol_short!("migrate"), total);
        total
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
            proposals_created: read_counter(env, PROPOSALS_CREATED),
            votes_cast: read_counter(env, VOTES_CAST),
            upgrades_executed: read_counter(env, UPGRADES_EXECUTED),
            migrations_run: read_counter(env, MIGRATIONS_RUN),
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
            proposals_created: read_counter(env, PROPOSALS_CREATED),
            votes_cast: read_counter(env, VOTES_CAST),
            upgrades_executed: read_counter(env, UPGRADES_EXECUTED),
            migrations_run: read_counter(env, MIGRATIONS_RUN),
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

// ============================================================================
// Contract Definition
// ============================================================================

#[contract]
pub struct GrainlifyContract;

// ============================================================================
// Data Structures
// ============================================================================

/// Storage keys for contract data.
///
/// # Keys
/// * `Admin` - Stores the administrator address (set once at initialization)
/// * `Version` - Stores the current contract version number
///
/// # Storage Type
/// Instance storage - Persists across contract upgrades
///
/// # Security Note
/// These keys use instance storage to ensure data survives WASM upgrades.
/// The admin address is immutable after initialization.
#[contracttype]
#[derive(Clone)]
enum DataKey {
    /// Administrator address with upgrade authority
    Admin,

    /// Current version number (increments with upgrades)
    Version,

    /// Migration state tracking - prevents double migration
    MigrationState,

    /// Previous version before migration (for rollback support)
    PreviousVersion,

    /// Configured single-admin upgrade delay in seconds
    UpgradeDelay,

    /// Pending single-admin upgrade schedule
    ScheduledUpgrade,
}

// ============================================================================
// Constants
// ============================================================================

/// Current contract version.
///
/// This constant should be incremented with each contract upgrade:
/// - MAJOR version: Breaking changes
/// - MINOR version: New features (backward compatible)
/// - PATCH version: Bug fixes
///
/// # Version History
/// - v1: Initial release with basic upgrade functionality
/// - v2: Added state migration system
///
/// # Usage
/// Set during initialization and can be updated via `set_version()`.
const VERSION: u32 = 2;

/// Minimum single-admin upgrade delay: 5 minutes.
const MIN_UPGRADE_DELAY_SECONDS: u64 = 5 * 60;

/// Default single-admin upgrade delay: 24 hours.
const DEFAULT_UPGRADE_DELAY_SECONDS: u64 = 24 * 60 * 60;

// ============================================================================
// Migration System
// ============================================================================

/// Migration state tracking to prevent double migration
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationState {
    /// Version that was migrated from
    pub from_version: u32,
    /// Version that was migrated to
    pub to_version: u32,
    /// Timestamp when migration completed
    pub migrated_at: u64,
    /// Migration hash for verification
    pub migration_hash: BytesN<32>,
}

/// Migration event data
#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationEvent {
    pub from_version: u32,
    pub to_version: u32,
    pub timestamp: u64,
    pub migration_hash: BytesN<32>,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Event version for versioned governance/upgrade events
pub const EVENT_VERSION: u32 = 1;

/// Event emitted when an upgrade is proposed (multisig version).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeProposed {
    pub version: u32,
    pub proposal_id: u64,
    pub proposer: Address,
    pub wasm_hash: BytesN<32>,
}

/// Event emitted when an upgrade proposal is approved (multisig version).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeApproved {
    pub version: u32,
    pub proposal_id: u64,
    pub signer: Address,
    pub approval_count: u32,
}

/// Event emitted when an upgrade is executed (both multisig and single-admin).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeExecuted {
    pub version: u32,
    pub proposal_id: Option<u64>,
    pub wasm_hash: BytesN<32>,
}

/// Event emitted when version number changes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionChanged {
    pub version: u32,
    pub old_version: u32,
    pub new_version: u32,
    pub admin: Address,
}

/// Event emitted when migration completes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationCompleted {
    pub version: u32,
    pub from_version: u32,
    pub to_version: u32,
    pub timestamp: u64,
    pub migration_hash: BytesN<32>,
    pub success: bool,
    pub error_message: Option<String>,
}


/// Pending single-admin upgrade data.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledUpgrade {
    pub wasm_hash: BytesN<32>,
    pub scheduled_at: u64,
    pub executable_at: u64,
}

/// Event emitted when a single-admin upgrade is scheduled.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeScheduledEvent {
    pub wasm_hash: BytesN<32>,
    pub scheduled_at: u64,
    pub executable_at: u64,
    pub delay_seconds: u64,
}

/// Event emitted when a scheduled single-admin upgrade is executed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeExecutedEvent {
    pub wasm_hash: BytesN<32>,
    pub executed_at: u64,
    pub previous_version: u32,
}

// ============================================================================
// Contract Implementation
// ============================================================================

// ========================================================================
// Initialization
// ========================================================================

/// Initializes the contract with an admin address.
///
/// # Arguments
/// * `env` - The contract environment
/// * `admin` - Address authorized to perform upgrades
///
/// # Panics
/// * If contract is already initialized
///
/// # State Changes
/// - Sets Admin address in instance storage
/// - Sets initial Version number
///
/// # Security Considerations
/// - Can only be called once (prevents admin takeover)
/// - Admin address is immutable after initialization
/// - Admin should be a secure address (hardware wallet/multi-sig)
/// - No authorization required for initialization (first-caller pattern)
///
/// # Example
/// ```rust,ignore
/// use soroban_sdk::{Address, Env};
///
/// let env = Env::default();
/// let admin = Address::generate(&env);
///
/// // Initialize contract
/// contract.init(&env, &admin);
///
/// // Subsequent init attempts will panic
/// // contract.init(&env, &another_admin); // ❌ Panics!
/// ```
///
/// # Gas Cost
/// Low - Two storage writes
///
/// # Production Deployment
/// ```bash
/// # Deploy contract
/// stellar contract deploy \
///   --wasm target/wasm32-unknown-unknown/release/grainlify.wasm \
///   --source ADMIN_SECRET_KEY
///
/// # Initialize with admin address
/// stellar contract invoke \
///   --id CONTRACT_ID \
///   --source ADMIN_SECRET_KEY \
///   -- init \
///   --admin GADMIN_ADDRESS
/// ```

#[contractimpl]
impl GrainlifyContract {
    /// Initializes the contract with multisig configuration.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `signers` - List of signer addresses for multisig
    /// * `threshold` - Number of signatures required to execute proposals
    pub fn init(env: Env, signers: Vec<Address>, threshold: u32) {
        if env.storage().instance().has(&DataKey::Version) {
            panic!("Already initialized");
        }

        MultiSig::init(&env, signers, threshold);
        env.storage().instance().set(&DataKey::Version, &VERSION);
    }

    /// Initialize governance system
    pub fn init_governance(
        env: Env,
        admin: Address,
        config: governance::GovernanceConfig,
    ) -> Result<(), governance::Error> {
        governance::GovernanceContract::init_governance(env, admin, config)
    }

    /// Create a governance proposal for a candidate upgrade WASM hash.
    ///
    /// On success the persistent `proposals_created` metric counter is incremented
    /// and a governance metric event is emitted. Counter tracking happens only
    /// after the governance module has accepted the proposal, so failed calls are
    /// never counted and the counter can never gate proposal creation.
    pub fn create_proposal(
        env: Env,
        proposer: Address,
        new_wasm_hash: BytesN<32>,
        description: Symbol,
    ) -> Result<u32, governance::Error> {
        let proposal_id = governance::GovernanceContract::create_proposal(
            env.clone(),
            proposer,
            new_wasm_hash,
            description,
        )?;
        monitoring::track_proposal_created(&env);
        Ok(proposal_id)
    }

    /// Cast a governance vote for a proposal.
    ///
    /// On success the persistent `votes_cast` metric counter is incremented and a
    /// governance metric event is emitted. Counter tracking happens only after the
    /// vote has been persisted, so rejected votes are never counted.
    pub fn cast_vote(
        env: Env,
        voter: Address,
        proposal_id: u32,
        vote_type: governance::VoteType,
    ) -> Result<(), governance::Error> {
        governance::GovernanceContract::cast_vote(env.clone(), voter, proposal_id, vote_type)?;
        monitoring::track_vote_cast(&env);
        Ok(())
    }

    /// Finalize voting and move the proposal to Approved or Rejected.
    pub fn finalize_proposal(
        env: Env,
        proposal_id: u32,
    ) -> Result<governance::ProposalStatus, governance::Error> {
        governance::GovernanceContract::finalize_proposal(env, proposal_id)
    }

    /// Mark an approved governance proposal as executed after its delay.
    pub fn execute_proposal(env: Env, proposal_id: u32) -> Result<(), governance::Error> {
        governance::GovernanceContract::execute_proposal(env, proposal_id)
    }

    /// Query whether governance executed an upgrade proposal for `wasm_hash`.
    pub fn is_upg_ok(env: Env, wasm_hash: BytesN<32>) -> bool {
        governance::GovernanceContract::is_upgrade_approved(env, wasm_hash)
    }

    /// Initializes the contract with a single admin address.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `admin` - Address authorized to perform upgrades
    pub fn init_admin(env: Env, admin: Address) {
        let start = env.ledger().timestamp();

        // Prevent re-initialization to protect admin immutability
        if env.storage().instance().has(&DataKey::Admin) {
            monitoring::track_operation(&env, symbol_short!("init"), admin.clone(), false);
            panic!("Already initialized");
        }

        // Store admin address (immutable after this point)
        env.storage().instance().set(&DataKey::Admin, &admin);

        // Set initial version
        env.storage().instance().set(&DataKey::Version, &VERSION);

        // Track successful operation
        monitoring::track_operation(&env, symbol_short!("init"), admin, true);

        // Track performance
        let duration = env.ledger().timestamp().saturating_sub(start);
        monitoring::emit_performance(&env, symbol_short!("init"), duration);
    }

    /// Proposes an upgrade with a new WASM hash (multisig version).
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `proposer` - Address proposing the upgrade
    /// * `wasm_hash` - Hash of the new WASM code
    ///
    /// # Returns
    /// * `u64` - The proposal ID
    pub fn propose_upgrade(env: Env, proposer: Address, wasm_hash: BytesN<32>) -> u64 {
        MultiSig::propose(&env, proposer, ProposalAction::Upgrade(wasm_hash))
    }

    /// Approves an upgrade proposal (multisig version).
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `proposal_id` - The ID of the proposal to approve
    /// * `signer` - Address approving the proposal
    pub fn approve_upgrade(env: Env, proposal_id: u64, signer: Address) {
        MultiSig::approve(&env, proposal_id, signer);
    }

    /// Remove a signer from the multisig configuration.
    ///
    /// The removal is guarded by a threshold-viability check: if removing the
    /// signer would leave fewer signers than the configured approval threshold
    /// (making it impossible to ever reach quorum), the call is rejected with a
    /// typed `RemovalWouldBreakThreshold` error.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `caller` - Address authorising the removal (must sign the transaction)
    /// * `signer_to_remove` - The signer address to remove from the set
    pub fn remove_signer(env: Env, caller: Address, signer_to_remove: Address) {
        MultiSig::remove_signer(&env, caller, signer_to_remove);
    }

    /// Returns the configured single-admin upgrade delay in seconds.
    pub fn get_upgrade_delay(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::UpgradeDelay)
            .unwrap_or(DEFAULT_UPGRADE_DELAY_SECONDS)
    }

    /// Updates the single-admin upgrade delay.
    ///
    /// The delay must be at least `MIN_UPGRADE_DELAY_SECONDS` to preserve a
    /// review window between scheduling and execution.
    pub fn set_upgrade_delay(env: Env, delay_seconds: u64) {
        let start = env.ledger().timestamp();
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        if delay_seconds < MIN_UPGRADE_DELAY_SECONDS {
            panic!("Upgrade delay below minimum");
        }

        env.storage()
            .instance()
            .set(&DataKey::UpgradeDelay, &delay_seconds);

        monitoring::track_operation(&env, symbol_short!("set_delay"), admin, true);
        let duration = env.ledger().timestamp().saturating_sub(start);
        monitoring::emit_performance(&env, symbol_short!("set_delay"), duration);
    }

    /// Schedules a single-admin upgrade for execution after the configured delay.
    pub fn schedule_upgrade(env: Env, wasm_hash: BytesN<32>) -> ScheduledUpgrade {
        let start = env.ledger().timestamp();
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let delay_seconds = Self::get_upgrade_delay(env.clone());
        let scheduled_at = env.ledger().timestamp();
        let executable_at = scheduled_at.saturating_add(delay_seconds);
        let scheduled = ScheduledUpgrade {
            wasm_hash: wasm_hash.clone(),
            scheduled_at,
            executable_at,
        };

        env.storage()
            .instance()
            .set(&DataKey::ScheduledUpgrade, &scheduled);

        env.events().publish(
            (symbol_short!("upg_sch"),),
            UpgradeScheduledEvent {
                wasm_hash,
                scheduled_at,
                executable_at,
                delay_seconds,
            },
        );

        monitoring::track_operation(&env, symbol_short!("sched_upg"), admin, true);
        let duration = env.ledger().timestamp().saturating_sub(start);
        monitoring::emit_performance(&env, symbol_short!("sched_upg"), duration);

        scheduled
    }

    /// Returns the active scheduled single-admin upgrade, if one exists.
    pub fn get_scheduled_upgrade(env: Env) -> Option<ScheduledUpgrade> {
        env.storage().instance().get(&DataKey::ScheduledUpgrade)
    }

    /// Returns whether `wasm_hash` matches the active schedule and is executable now.
    pub fn is_upgrade_ready(env: Env, wasm_hash: BytesN<32>) -> bool {
        let Some(scheduled) = Self::get_scheduled_upgrade(env.clone()) else {
            return false;
        };

        scheduled.wasm_hash == wasm_hash && env.ledger().timestamp() >= scheduled.executable_at
    }

    /// Upgrades the contract to new WASM code.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `new_wasm_hash` - Hash of the uploaded WASM code (32 bytes)
    ///
    /// # Authorization
    /// - **CRITICAL**: Only admin can call this function
    /// - Admin must sign the transaction
    ///
    /// # State Changes
    /// - Replaces current contract WASM with new version
    /// - Preserves all instance storage (admin, version, etc.)
    /// - Does NOT automatically update version number (call `set_version` separately)
    ///
    /// # Security Considerations
    /// - **Code Review**: New WASM must be audited before deployment
    /// - **Testing**: Test upgrade on testnet first
    /// - **State Compatibility**: Ensure new code is compatible with existing state
    /// - **Rollback Plan**: Keep previous WASM hash for emergency rollback
    /// - **Version Update**: Call `set_version` after upgrade if needed
    ///
    /// # Workflow
    /// 1. Develop and test new contract version
    /// 2. Build WASM: `cargo build --release --target wasm32-unknown-unknown`
    /// 3. Upload WASM to Stellar network
    /// 4. Get WASM hash from upload response
    /// 5. Call `schedule_upgrade` with the hash
    /// 6. Wait until the scheduled `executable_at` timestamp
    /// 7. Call this function with the same hash
    /// 8. (Optional) Call `set_version` to update version number
    ///
    /// # Example
    /// ```rust,ignore
    /// use soroban_sdk::{BytesN, Env};
    ///
    /// let env = Env::default();
    ///
    /// // Upload new WASM and get hash (done off-chain)
    /// let wasm_hash = BytesN::from_array(
    ///     &env,
    ///     &[0xab, 0xcd, 0xef, ...] // 32 bytes
    /// );
    ///
    /// // Schedule upgrade, wait for executable_at, then perform upgrade
    /// let scheduled = contract.schedule_upgrade(&env, &wasm_hash);
    /// // Wait until ledger timestamp >= scheduled.executable_at
    /// contract.upgrade(&env, &wasm_hash);
    ///
    /// // Update version number
    /// contract.set_version(&env, &2);
    /// ```
    ///
    /// # Production Upgrade Process
    /// ```bash
    /// # 1. Build new WASM
    /// cargo build --release --target wasm32-unknown-unknown
    ///
    /// # 2. Upload WASM to Stellar
    /// stellar contract install \
    ///   --wasm target/wasm32-unknown-unknown/release/grainlify.wasm \
    ///   --source ADMIN_SECRET_KEY
    /// # Output: WASM_HASH (e.g., abc123...)
    ///
    /// # 3. Schedule upgrade
    /// stellar contract invoke \
    ///   --id CONTRACT_ID \
    ///   --source ADMIN_SECRET_KEY \
    ///   -- schedule_upgrade \
    ///   --wasm_hash WASM_HASH
    ///
    /// # 4. Wait until the returned executable_at timestamp
    ///
    /// # 5. Upgrade contract
    /// stellar contract invoke \
    ///   --id CONTRACT_ID \
    ///   --source ADMIN_SECRET_KEY \
    ///   -- upgrade \
    ///   --new_wasm_hash WASM_HASH
    ///
    /// # 6. Update version (optional)
    /// stellar contract invoke \
    ///   --id CONTRACT_ID \
    ///   --source ADMIN_SECRET_KEY \
    ///   -- set_version \
    ///   --new_version 2
    /// ```
    ///
    /// # Gas Cost
    /// High - WASM code replacement is expensive
    ///
    /// # Emergency Rollback
    /// If new version has issues, schedule the previous WASM hash and execute it
    /// after the configured timelock:
    /// ```bash
    /// stellar contract invoke \
    ///   --id CONTRACT_ID \
    ///   --source ADMIN_SECRET_KEY \
    ///   -- schedule_upgrade \
    ///   --wasm_hash PREVIOUS_WASM_HASH
    ///
    /// stellar contract invoke \
    ///   --id CONTRACT_ID \
    ///   --source ADMIN_SECRET_KEY \
    ///   -- upgrade \
    ///   --new_wasm_hash PREVIOUS_WASM_HASH
    /// ```
    ///
    /// # Panics
    /// * If admin address is not set (contract not initialized)
    /// * If caller is not the admin

    /// Executes an upgrade proposal that has met the multisig threshold.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `proposal_id` - The ID of the upgrade proposal to execute
    /// * `expected_nonce` - The current execution nonce (see [`Self::multisig_nonce`]).
    ///   Execution is rejected with `NonceMismatch` if this does not match,
    ///   providing replay protection.
    pub fn execute_upgrade(env: Env, proposal_id: u64, expected_nonce: u64) {
        let action = MultiSig::get_action(&env, proposal_id);
        let wasm_hash = match action.clone() {
            ProposalAction::Upgrade(wasm_hash) => wasm_hash,
        };
        let upgrade_env = env.clone();

        MultiSig::execute(&env, proposal_id, action, expected_nonce, || {
            upgrade_env
                .deployer()
                .update_current_contract_wasm(wasm_hash.clone());
        });

        env.events().publish(
            (symbol_short!("upg_exec2"),),
            UpgradeExecuted {
                version: EVENT_VERSION,
                proposal_id: Some(proposal_id),
                wasm_hash,
            },
        );

        // Observational metric only: recorded after the upgrade has been applied.
        monitoring::track_upgrade_executed(&env);
    }

    /// Returns the next multisig execution nonce.
    ///
    /// This value must be passed as `expected_nonce` to [`Self::execute_upgrade`].
    /// It increments after every successful execution, so a previously used
    /// nonce (and its collected approvals) can never be replayed.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    pub fn multisig_nonce(env: Env) -> u64 {
        MultiSig::nonce(&env)
    }

    /// Upgrades the contract to new WASM code (single admin version).
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `new_wasm_hash` - Hash of the uploaded WASM code (32 bytes)
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        let start = env.ledger().timestamp();

        // Verify admin authorization
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let scheduled = require_scheduled_upgrade(&env, &new_wasm_hash);

        // Store previous version for potential rollback
        let current_version = env.storage().instance().get(&DataKey::Version).unwrap_or(1);
        env.storage()
            .instance()
            .set(&DataKey::PreviousVersion, &current_version);

        // Perform WASM upgrade
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());

        env.storage().instance().remove(&DataKey::ScheduledUpgrade);

        env.events().publish(
            (symbol_short!("upg_exec"),),
            UpgradeExecutedEvent {
                wasm_hash: scheduled.wasm_hash.clone(),
                executed_at: env.ledger().timestamp(),
                previous_version: current_version,
            },
        );

        env.events().publish(
            (symbol_short!("upg_exec2"),),
            UpgradeExecuted {
                version: EVENT_VERSION,
                proposal_id: None,
                wasm_hash: scheduled.wasm_hash,
            },
        );

        // Track successful operation
        monitoring::track_operation(&env, symbol_short!("upgrade"), admin, true);

        // Record the upgrade in the persistent governance/upgrade metric counter.
        monitoring::track_upgrade_executed(&env);

        // Track performance
        let duration = env.ledger().timestamp().saturating_sub(start);
        monitoring::emit_performance(&env, symbol_short!("upgrade"), duration);
    }

    // ========================================================================
    // Version Management
    // ========================================================================

    /// Retrieves the current contract version number.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    ///
    /// # Returns
    /// * `u32` - Current version number (defaults to 0 if not set)
    ///
    /// # Usage
    /// Use this to verify contract version for:
    /// - Client compatibility checks
    /// - Migration decision logic
    /// - Audit trails
    /// - Version-specific behavior
    ///
    /// # Example
    /// ```rust,ignore
    /// let version = contract.get_version(&env);
    ///
    /// match version {
    ///     1 => println!("Running v1"),
    ///     2 => println!("Running v2 with new features"),
    ///     _ => println!("Unknown version"),
    /// }
    /// ```
    ///
    /// # Client-Side Usage
    /// ```javascript
    /// // Check contract version before interaction
    /// const version = await contract.get_version();
    ///
    /// if (version < 2) {
    ///     throw new Error("Contract version too old, please upgrade");
    /// }
    /// ```
    ///
    /// # Gas Cost
    /// Very Low - Single storage read
    pub fn get_version(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::Version).unwrap_or(0)
    }

    /// Alias for get_version, exposed for off-chain tooling. Returns the current stored version.
    pub fn version(env: Env) -> u32 {
        Self::get_version(env)
    }

    /// Short alias used by escrow governance integration cross-contract calls.
    pub fn get_ver(env: Env) -> u32 {
        Self::get_version(env)
    }

    /// Returns the semantic version string (e.g., "1.0.0").
    /// Falls back to mapping known numeric values to semantic strings.
    pub fn get_version_semver_string(env: Env) -> String {
        let raw: u32 = env.storage().instance().get(&DataKey::Version).unwrap_or(0);
        let s = match raw {
            0 => "0.0.0",
            1 | 10000 => "1.0.0",
            2 | 20000 => "2.0.0",
            10100 => "1.1.0",
            10001 => "1.0.1",
            _ => "unknown",
        };
        String::from_str(&env, s)
    }

    /// Returns the numeric encoded semantic version using policy major*10_000 + minor*100 + patch.
    /// If the stored version is a simple major number (1,2,3...), it will be converted to major*10_000.
    pub fn get_version_numeric_encoded(env: Env) -> u32 {
        let raw: u32 = env.storage().instance().get(&DataKey::Version).unwrap_or(0);
        if raw >= 10_000 {
            raw
        } else {
            raw.saturating_mul(10_000)
        }
    }

    /// Ensures the current version meets a minimum required encoded semantic version.
    /// Panics if current version is lower than `min_numeric`.
    pub fn require_min_version(env: Env, min_numeric: u32) {
        let cur = Self::get_version_numeric_encoded(env.clone());
        if cur < min_numeric {
            panic!("Incompatible contract version");
        }
    }

    /// Updates the contract version number.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `new_version` - New version number to set
    ///
    /// # Authorization
    /// - Only admin can call this function
    /// - Admin must sign the transaction
    ///
    /// # State Changes
    /// - Updates Version in instance storage
    ///
    /// # Usage
    /// Call this function after upgrading contract WASM to reflect
    /// the new version number. This provides an audit trail of upgrades.
    ///
    /// # Version Numbering Strategy
    /// Recommend using semantic versioning encoded as single u32:
    /// - `1` = v1.0.0
    /// - `2` = v2.0.0
    /// - `101` = v1.0.1 (patch)
    /// - `110` = v1.1.0 (minor)
    ///
    /// Or use simple incrementing:
    /// - `1` = First version
    /// - `2` = Second version
    /// - `3` = Third version
    ///
    /// # Example
    /// ```rust,ignore
    /// // After upgrading WASM
    /// contract.upgrade(&env, &new_wasm_hash);
    ///
    /// // Update version to reflect the upgrade
    /// contract.set_version(&env, &2);
    ///
    /// // Verify
    /// assert_eq!(contract.get_version(&env), 2);
    /// ```
    ///
    /// # Best Practice
    /// Document version changes:
    /// ```rust,ignore
    /// // Version History:
    /// // 1 - Initial release
    /// // 2 - Added feature X, fixed bug Y
    /// // 3 - Performance improvements
    /// contract.set_version(&env, &3);
    /// ```
    ///
    /// # Security Note
    /// This function does NOT perform the actual upgrade.
    /// It only updates the version metadata. Always call
    /// `upgrade()` first, then `set_version()`.
    ///
    /// # Gas Cost
    /// Very Low - Single storage write
    ///
    /// # Panics
    /// * If admin address is not set (contract not initialized)
    /// * If caller is not the admin

    pub fn set_version(env: Env, new_version: u32) {
        let start = env.ledger().timestamp();

        // Verify admin authorization
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let old_version = env.storage().instance().get(&DataKey::Version).unwrap_or(1);

        // Update version number
        env.storage()
            .instance()
            .set(&DataKey::Version, &new_version);

        env.events().publish(
            (symbol_short!("ver_chg"),),
            VersionChanged {
                version: EVENT_VERSION,
                old_version,
                new_version,
                admin: admin.clone(),
            },
        );

        // Track successful operation
        monitoring::track_operation(&env, symbol_short!("set_ver"), admin, true);

        // Track performance
        let duration = env.ledger().timestamp().saturating_sub(start);
        monitoring::emit_performance(&env, symbol_short!("set_ver"), duration);
    }

    // ========================================================================
    // Monitoring & Analytics Functions
    // ========================================================================

    /// Health check - returns contract health status
    pub fn health_check(env: Env) -> monitoring::HealthStatus {
        monitoring::health_check(&env)
    }

    /// Get analytics - returns usage analytics
    pub fn get_analytics(env: Env) -> monitoring::Analytics {
        monitoring::get_analytics(&env)
    }

    /// Get state snapshot - returns current state
    pub fn get_state_snapshot(env: Env) -> monitoring::StateSnapshot {
        monitoring::get_state_snapshot(&env)
    }

    /// Get performance stats for a function
    pub fn get_performance_stats(env: Env, function_name: Symbol) -> monitoring::PerformanceStats {
        monitoring::get_performance_stats(&env, function_name)
    }

    // ========================================================================
    // State Migration System
    // ========================================================================

    /// Executes state migration from current version to target version.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `target_version` - Version to migrate to
    /// * `migration_hash` - Hash of migration data for verification
    ///
    /// # Authorization
    /// - Only admin can call this function
    /// - Admin must sign the transaction
    ///
    /// # State Changes
    /// - Migrates contract state from current version to target version
    /// - Updates version number
    /// - Records migration state to prevent double migration
    /// - Emits migration event
    ///
    /// # Migration Process
    /// 1. Validates current version and target version
    /// 2. Checks if migration already completed
    /// 3. Executes version-specific migration functions
    /// 4. Updates version number
    /// 5. Records migration state
    /// 6. Emits migration event
    ///
    /// # Example
    /// ```rust,ignore
    /// // After upgrading WASM to v2
    /// contract.upgrade(&env, &new_wasm_hash);
    ///
    /// // Migrate state from v1 to v2
    /// let migration_hash = BytesN::from_array(&env, &[...]);
    /// contract.migrate(&env, &2, &migration_hash);
    /// ```
    pub fn migrate(env: Env, target_version: u32, migration_hash: BytesN<32>) {
        let start = env.ledger().timestamp();

        // Verify admin authorization
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        // Get current version
        let current_version = env.storage().instance().get(&DataKey::Version).unwrap_or(1);

        // Check if migration already completed (idempotency check first)
        if env.storage().instance().has(&DataKey::MigrationState) {
            let migration_state: MigrationState = env
                .storage()
                .instance()
                .get(&DataKey::MigrationState)
                .unwrap();

            if migration_state.to_version >= target_version {
                // Migration already completed, skip
                return;
            }
        }

        // Validate target version
        if target_version <= current_version {
            let error_msg =
                String::from_str(&env, "Target version must be greater than current version");
            emit_migration_event(
                &env,
                MigrationEvent {
                    from_version: current_version,
                    to_version: target_version,
                    timestamp: env.ledger().timestamp(),
                    migration_hash,
                    success: false,
                    error_message: Some(error_msg),
                },
            );
            panic!("Target version must be greater than current version");
        }

        // Check if migration already completed
        if env.storage().instance().has(&DataKey::MigrationState) {
            let migration_state: MigrationState = env
                .storage()
                .instance()
                .get(&DataKey::MigrationState)
                .unwrap();

            if migration_state.to_version >= target_version {
                // Migration already completed, skip
                return;
            }
        }

        // Execute version-specific migrations
        let mut from_version = current_version;
        while from_version < target_version {
            let next_version = from_version + 1;

            // Execute migration from from_version to next_version
            match next_version {
                2 => migrate_v1_to_v2(&env),
                3 => migrate_v2_to_v3(&env),
                _ => {
                    let error_msg = String::from_str(&env, "No migration path available");
                    emit_migration_event(
                        &env,
                        MigrationEvent {
                            from_version,
                            to_version: next_version,
                            timestamp: env.ledger().timestamp(),
                            migration_hash: migration_hash.clone(),
                            success: false,
                            error_message: Some(error_msg),
                        },
                    );
                    panic!("No migration path available");
                }
            }

            from_version = next_version;
        }

        // Update version
        env.storage()
            .instance()
            .set(&DataKey::Version, &target_version);

        // Record migration state
        let migration_state = MigrationState {
            from_version: current_version,
            to_version: target_version,
            migrated_at: env.ledger().timestamp(),
            migration_hash: migration_hash.clone(),
        };
        env.storage()
            .instance()
            .set(&DataKey::MigrationState, &migration_state);

        // Emit success event
        emit_migration_event(
            &env,
            MigrationEvent {
                from_version: current_version,
                to_version: target_version,
                timestamp: env.ledger().timestamp(),
                migration_hash: migration_hash.clone(),
                success: true,
                error_message: None,
            },
        );

        // Track successful operation
        monitoring::track_operation(&env, symbol_short!("migrate"), admin, true);

        // Record the migration in the persistent governance/upgrade metric counter.
        // Idempotent re-invocations return early above, so this counts each
        // applied migration exactly once.
        monitoring::track_migration_run(&env);

        // Track performance
        let duration = env.ledger().timestamp().saturating_sub(start);
        monitoring::emit_performance(&env, symbol_short!("migrate"), duration);
    }

    /// Gets the current migration state.
    ///
    /// # Returns
    /// * `Option<MigrationState>` - Current migration state if exists
    pub fn get_migration_state(env: Env) -> Option<MigrationState> {
        if env.storage().instance().has(&DataKey::MigrationState) {
            Some(
                env.storage()
                    .instance()
                    .get(&DataKey::MigrationState)
                    .unwrap(),
            )
        } else {
            None
        }
    }

    /// Gets the previous version (before last upgrade).
    ///
    /// # Returns
    /// * `Option<u32>` - Previous version if exists
    pub fn get_previous_version(env: Env) -> Option<u32> {
        if env.storage().instance().has(&DataKey::PreviousVersion) {
            Some(
                env.storage()
                    .instance()
                    .get(&DataKey::PreviousVersion)
                    .unwrap(),
            )
        } else {
            None
        }
    }
}

// ============================================================================
// Migration Functions
// ============================================================================

/// Emits a migration event for audit trail
fn emit_migration_event(env: &Env, event: MigrationEvent) {
    env.events().publish((symbol_short!("migration"),), event.clone());

    env.events().publish(
        (symbol_short!("mig_comp"),),
        MigrationCompleted {
            version: EVENT_VERSION,
            from_version: event.from_version,
            to_version: event.to_version,
            timestamp: event.timestamp,
            migration_hash: event.migration_hash,
            success: event.success,
            error_message: event.error_message,
        },
    );
}

fn require_scheduled_upgrade(env: &Env, wasm_hash: &BytesN<32>) -> ScheduledUpgrade {
    let scheduled: ScheduledUpgrade = env
        .storage()
        .instance()
        .get(&DataKey::ScheduledUpgrade)
        .unwrap_or_else(|| panic!("No scheduled upgrade"));

    if scheduled.wasm_hash != wasm_hash.clone() {
        panic!("Scheduled upgrade hash mismatch");
    }

    if env.ledger().timestamp() < scheduled.executable_at {
        panic!("Upgrade timelock not elapsed");
    }

    scheduled
}

/// Migration from version 1 to version 2
/// This is a placeholder migration - add actual data transformation logic here
fn migrate_v1_to_v2(_env: &Env) {
    // Example: Transform old data structures to new ones
    // This is where you would:
    // 1. Read old data format
    // 2. Transform to new format
    // 3. Write new data format
    // 4. Clean up old data if needed

    // For now, this is a no-op migration
    // Add actual migration logic based on your data structure changes
}

/// Migration from version 2 to version 3
/// Placeholder for future migrations
fn migrate_v2_to_v3(_env: &Env) {
    // Future migration logic here
    // This will be implemented when v3 is released
}

// ============================================================================
// Testing Module
// ============================================================================
#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Events, Ledger};
    use soroban_sdk::{testutils::Address as _, Env, IntoVal, TryFromVal};

    #[test]
    fn multisig_init_works() {
        let env = Env::default();
        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let mut signers = soroban_sdk::Vec::new(&env);
        signers.push_back(Address::generate(&env));
        signers.push_back(Address::generate(&env));
        signers.push_back(Address::generate(&env));

        client.init(&signers, &2u32);
    }

    #[test]
    #[should_panic(expected = "Already initialized")]
    fn test_init_twice_panics() {
        let env = Env::default();
        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let mut signers = soroban_sdk::Vec::new(&env);
        signers.push_back(Address::generate(&env));
        signers.push_back(Address::generate(&env));
        signers.push_back(Address::generate(&env));

        client.init(&signers, &2u32);
        client.init(&signers, &2u32);
    }

    #[test]
    fn test_set_version() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        client.set_version(&2);
        assert_eq!(client.get_version(), 2);
    }

    #[test]
    fn test_schedule_upgrade_records_timelock() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = 1_000);

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let wasm_hash = BytesN::from_array(&env, &[9u8; 32]);
        client.set_upgrade_delay(&600);
        let scheduled = client.schedule_upgrade(&wasm_hash);

        assert_eq!(scheduled.wasm_hash, wasm_hash);
        assert_eq!(scheduled.scheduled_at, 1_000);
        assert_eq!(scheduled.executable_at, 1_600);
        assert_eq!(client.get_scheduled_upgrade().unwrap(), scheduled);
    }

    #[test]
    #[should_panic(expected = "No scheduled upgrade")]
    fn test_upgrade_requires_prior_schedule() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let wasm_hash = BytesN::from_array(&env, &[8u8; 32]);
        client.upgrade(&wasm_hash);
    }

    #[test]
    #[should_panic(expected = "Upgrade timelock not elapsed")]
    fn test_upgrade_rejects_early_execution() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = 2_000);

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let wasm_hash = BytesN::from_array(&env, &[7u8; 32]);
        client.set_upgrade_delay(&600);
        client.schedule_upgrade(&wasm_hash);

        env.ledger().with_mut(|li| li.timestamp = 2_599);
        client.upgrade(&wasm_hash);
    }

    #[test]
    #[should_panic(expected = "Scheduled upgrade hash mismatch")]
    fn test_upgrade_rejects_hash_mismatch() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = 3_000);

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let scheduled_hash = BytesN::from_array(&env, &[1u8; 32]);
        let other_hash = BytesN::from_array(&env, &[2u8; 32]);
        client.set_upgrade_delay(&600);
        client.schedule_upgrade(&scheduled_hash);

        env.ledger().with_mut(|li| li.timestamp = 3_600);
        client.upgrade(&other_hash);
    }

    #[test]
    fn test_upgrade_ready_at_exact_boundary() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = 4_000);

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let wasm_hash = BytesN::from_array(&env, &[3u8; 32]);
        client.set_upgrade_delay(&600);
        client.schedule_upgrade(&wasm_hash);

        env.ledger().with_mut(|li| li.timestamp = 4_599);
        assert!(!client.is_upgrade_ready(&wasm_hash));

        env.ledger().with_mut(|li| li.timestamp = 4_600);
        assert!(client.is_upgrade_ready(&wasm_hash));
    }

    #[test]
    fn test_upgrade_executes_after_timelock() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = 5_000);

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let wasm_hash = env.deployer().upload_contract_wasm([].as_slice());
        client.set_upgrade_delay(&600);
        client.schedule_upgrade(&wasm_hash);

        let events_before_upgrade = env.events().all().len();
        env.ledger().with_mut(|li| li.timestamp = 5_600);

        client.upgrade(&wasm_hash);

        assert!(env.events().all().len() > events_before_upgrade);
    }

    #[test]
    fn test_migration_v1_to_v2() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        // Initial version is 2
        assert_eq!(client.get_version(), 2);

        // Create migration hash
        let migration_hash = BytesN::from_array(&env, &[0u8; 32]);

        // Migrate to version 3
        client.migrate(&3, &migration_hash);

        // Verify version updated
        assert_eq!(client.get_version(), 3);

        // Verify migration state recorded
        let migration_state = client.get_migration_state();
        assert!(migration_state.is_some());
        let state = migration_state.unwrap();
        assert_eq!(state.from_version, 2);
        assert_eq!(state.to_version, 3);
    }

    #[test]
    #[should_panic(expected = "Target version must be greater than current version")]
    fn test_migration_invalid_target_version() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let migration_hash = BytesN::from_array(&env, &[0u8; 32]);

        // Try to migrate to version 1 when already at version 1
        client.migrate(&1, &migration_hash);
    }

    #[test]
    fn test_migration_idempotency() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let migration_hash = BytesN::from_array(&env, &[0u8; 32]);

        // Migrate to version 3
        client.migrate(&3, &migration_hash);
        assert_eq!(client.get_version(), 3);

        // Try to migrate again - should be idempotent
        client.migrate(&3, &migration_hash);
        assert_eq!(client.get_version(), 3);

        // Verify migration state unchanged
        let migration_state = client.get_migration_state();
        assert!(migration_state.is_some());
        let state = migration_state.unwrap();
        assert_eq!(state.to_version, 3);
    }

    #[test]
    fn test_get_previous_version() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        // Initially no previous version
        assert!(client.get_previous_version().is_none());

        // Simulate upgrade (this would normally be done via upgrade() but we'll set version directly)
        client.set_version(&2);

        // Previous version should still be None unless upgrade() was called
        // This test verifies the get_previous_version function works
    }

    // ========================================================================
    // Integration Tests: Upgrade and Migration Workflow
    // ========================================================================

    #[test]
    fn test_complete_upgrade_and_migration_workflow() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);

        // 1. Initialize contract
        client.init_admin(&admin);
        assert_eq!(client.get_version(), 2);

        // 2. Simulate upgrade (in real scenario, this would call upgrade() with WASM hash)
        // For testing, we'll just test the migration part
        let migration_hash = BytesN::from_array(&env, &[1u8; 32]);

        // 3. Migrate to version 3
        client.migrate(&3, &migration_hash);

        // 4. Verify version updated
        assert_eq!(client.get_version(), 3);

        // 5. Verify migration state recorded
        let migration_state = client.get_migration_state();
        assert!(migration_state.is_some());
        let state = migration_state.unwrap();
        assert_eq!(state.from_version, 2);
        assert_eq!(state.to_version, 3);
        assert!(state.migrated_at >= 0);

        // 6. Verify events emitted
        let events = env.events().all();
        assert!(events.len() > 0);
    }

    #[test]
    fn test_migration_sequential_versions() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        // Migrate from v2 to v3
        let hash1 = BytesN::from_array(&env, &[1u8; 32]);
        client.migrate(&3, &hash1);
        assert_eq!(client.get_version(), 3);

        // Could test v3 to v4 if that migration path existed
        // For now, verify v2->v3 worked
        let state = client.get_migration_state().unwrap();
        assert_eq!(state.from_version, 2);
        assert_eq!(state.to_version, 3);
    }

    #[test]
    fn test_migration_event_emission() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let initial_event_count = env.events().all().len();

        let migration_hash = BytesN::from_array(&env, &[2u8; 32]);
        client.migrate(&3, &migration_hash);

        // Verify migration event was emitted
        let events = env.events().all();
        assert!(events.len() > initial_event_count);
    }

    #[test]
    fn test_admin_initialization() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        assert_eq!(client.get_version(), 2);
    }

    #[test]
    #[should_panic(expected = "Already initialized")]
    fn test_cannot_reinitialize_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin1 = Address::generate(&env);
        let admin2 = Address::generate(&env);

        client.init_admin(&admin1);
        client.init_admin(&admin2);
    }

    #[test]
    fn test_admin_persists_across_version_updates() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        client.set_version(&3);
        assert_eq!(client.get_version(), 3);

        client.set_version(&4);
        assert_eq!(client.get_version(), 4);
    }

    // ========================================================================
    // Migration Hook Tests (Issue #45)
    // ========================================================================

    #[test]
    fn test_migration_only_runs_once_per_version() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        // Verify initial version
        assert_eq!(client.get_version(), 2);

        // Migrate to v3
        let hash = BytesN::from_array(&env, &[1u8; 32]);
        client.migrate(&3, &hash);

        let state1 = client.get_migration_state().unwrap();
        let timestamp1 = state1.migrated_at;

        // Second call with same version - should be idempotent (not re-execute)
        client.migrate(&3, &hash);
        let state2 = client.get_migration_state().unwrap();

        // Verify state unchanged (migration not re-executed)
        assert_eq!(state2.migrated_at, timestamp1);
        assert_eq!(state2.to_version, 3);
    }

    #[test]
    fn test_migration_transforms_state_correctly() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let initial_version = client.get_version();
        assert_eq!(initial_version, 2);

        let hash = BytesN::from_array(&env, &[2u8; 32]);

        // Execute migration to v3
        client.migrate(&3, &hash);

        // Verify transformations
        assert_eq!(client.get_version(), 3);

        let state = client.get_migration_state().unwrap();
        assert_eq!(state.from_version, initial_version);
        assert_eq!(state.to_version, 3);
        assert_eq!(state.migration_hash, hash);
        // Timestamp is set (may be 0 in test environment)
        assert!(state.migrated_at >= 0);
    }

    #[test]
    fn test_migration_requires_admin_authorization() {
        let env = Env::default();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        env.mock_all_auths_allowing_non_root_auth();

        let hash = BytesN::from_array(&env, &[3u8; 32]);

        // This should require admin auth
        client.migrate(&3, &hash);

        // Verify auth was required
        assert!(env.auths().len() > 0);
    }

    #[test]
    #[should_panic(expected = "Target version must be greater than current version")]
    fn test_migration_rejects_downgrade() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        client.set_version(&4);

        let hash = BytesN::from_array(&env, &[4u8; 32]);

        // Try to migrate to lower version - should panic
        client.migrate(&3, &hash);
    }

    #[test]
    fn test_migration_state_persists() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let hash = BytesN::from_array(&env, &[5u8; 32]);
        client.migrate(&3, &hash);

        // Retrieve state multiple times
        let state1 = client.get_migration_state().unwrap();
        let state2 = client.get_migration_state().unwrap();

        assert_eq!(state1.from_version, state2.from_version);
        assert_eq!(state1.to_version, state2.to_version);
        assert_eq!(state1.migrated_at, state2.migrated_at);
        assert_eq!(state1.migration_hash, state2.migration_hash);
    }

    #[test]
    fn test_migration_emits_success_event() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let initial_events = env.events().all().len();

        let hash = BytesN::from_array(&env, &[6u8; 32]);
        client.migrate(&3, &hash);

        let events = env.events().all();
        assert!(events.len() > initial_events);
    }

    #[test]
    fn test_migration_tracks_previous_version() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let v_before = client.get_version();
        assert_eq!(v_before, 2);

        let hash = BytesN::from_array(&env, &[7u8; 32]);
        client.migrate(&3, &hash);

        let state = client.get_migration_state().unwrap();
        assert_eq!(state.from_version, v_before);
        assert_eq!(state.to_version, 3);
    }

    // ========================================================================
    // Monitoring Counter Tests (Issue #101)
    //
    // These tests assert that the persistent governance/upgrade counters
    // increment exactly once per real operation, are surfaced through
    // get_analytics/get_state_snapshot, and emit a metric event. They also
    // confirm the counters are observational only and never gate control flow.
    // ========================================================================

    fn one_person_governance_config(env: &Env) -> GovernanceConfig {
        GovernanceConfig {
            voting_period: 100,
            execution_delay: 0,
            quorum_percentage: 1000,
            approval_threshold: 5000,
            min_proposal_stake: 0,
            voting_scheme: VotingScheme::OnePersonOneVote,
            governance_token: Address::generate(env),
            one_person_total_voters: 10,
            token_total_voting_power: 100,
            snapshot_ledger: None,
        }
    }

    #[test]
    fn test_counters_start_at_zero() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        client.init_admin(&Address::generate(&env));

        let analytics = client.get_analytics();
        assert_eq!(analytics.proposals_created, 0);
        assert_eq!(analytics.votes_cast, 0);
        assert_eq!(analytics.upgrades_executed, 0);
        assert_eq!(analytics.migrations_run, 0);
    }

    #[test]
    fn test_create_proposal_increments_counter() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let proposer = Address::generate(&env);
        client.init_governance(&admin, &one_person_governance_config(&env));

        let hash = BytesN::from_array(&env, &[1u8; 32]);
        client.create_proposal(&proposer, &hash, &symbol_short!("p1"));
        assert_eq!(client.get_analytics().proposals_created, 1);

        client.create_proposal(&proposer, &hash, &symbol_short!("p2"));
        let analytics = client.get_analytics();
        assert_eq!(analytics.proposals_created, 2);
        // Unrelated counters are untouched.
        assert_eq!(analytics.votes_cast, 0);
        assert_eq!(client.get_state_snapshot().proposals_created, 2);
    }

    #[test]
    fn test_create_proposal_emits_metric_event() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let proposer = Address::generate(&env);
        client.init_governance(&admin, &one_person_governance_config(&env));

        let before = env.events().all().len();
        let hash = BytesN::from_array(&env, &[2u8; 32]);
        client.create_proposal(&proposer, &hash, &symbol_short!("p1"));

        // The governance module does not emit events itself, so the new event
        // is the monitoring metric event.
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn test_cast_vote_increments_counter() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);
        client.init_governance(&admin, &one_person_governance_config(&env));

        let hash = BytesN::from_array(&env, &[3u8; 32]);
        let proposal_id = client.create_proposal(&proposer, &hash, &symbol_short!("p1"));

        client.cast_vote(&proposer, &proposal_id, &VoteType::For);
        client.cast_vote(&voter, &proposal_id, &VoteType::Against);

        let analytics = client.get_analytics();
        assert_eq!(analytics.votes_cast, 2);
        assert_eq!(analytics.proposals_created, 1);
        assert_eq!(client.get_state_snapshot().votes_cast, 2);
    }

    #[test]
    fn test_failed_vote_does_not_increment_counter() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let proposer = Address::generate(&env);
        client.init_governance(&admin, &one_person_governance_config(&env));

        let hash = BytesN::from_array(&env, &[4u8; 32]);
        let proposal_id = client.create_proposal(&proposer, &hash, &symbol_short!("p1"));

        client.cast_vote(&proposer, &proposal_id, &VoteType::For);
        // Double voting is rejected by governance; the counter must not move.
        let result = client.try_cast_vote(&proposer, &proposal_id, &VoteType::For);
        assert!(result.is_err());

        assert_eq!(client.get_analytics().votes_cast, 1);
    }

    #[test]
    fn test_migration_increments_counter_once() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        let hash = BytesN::from_array(&env, &[5u8; 32]);
        client.migrate(&3, &hash);
        assert_eq!(client.get_analytics().migrations_run, 1);

        // Idempotent re-invocation must not double-count the migration.
        client.migrate(&3, &hash);
        assert_eq!(client.get_analytics().migrations_run, 1);
        assert_eq!(client.get_state_snapshot().migrations_run, 1);
    }

    #[test]
    fn test_upgrade_counter_increments_and_persists() {
        // The single-admin/multisig upgrade paths replace the contract WASM, so
        // the helper is exercised directly inside the contract context here. This
        // verifies the counter is incremented, read back from persistent storage
        // (so it survives an upgrade), and surfaced through both views.
        let env = Env::default();
        let contract_id = env.register_contract(None, GrainlifyContract);

        env.as_contract(&contract_id, || {
            assert_eq!(monitoring::get_analytics(&env).upgrades_executed, 0);

            assert_eq!(monitoring::track_upgrade_executed(&env), 1);
            assert_eq!(monitoring::track_upgrade_executed(&env), 2);

            assert_eq!(monitoring::get_analytics(&env).upgrades_executed, 2);
            assert_eq!(monitoring::get_state_snapshot(&env).upgrades_executed, 2);
        });
    }

    #[test]
    fn test_counters_are_independent() {
        // Exercising one counter must never perturb the others.
        let env = Env::default();
        let contract_id = env.register_contract(None, GrainlifyContract);

        env.as_contract(&contract_id, || {
            monitoring::track_proposal_created(&env);
            monitoring::track_vote_cast(&env);
            monitoring::track_vote_cast(&env);
            monitoring::track_upgrade_executed(&env);
            monitoring::track_migration_run(&env);

            let analytics = monitoring::get_analytics(&env);
            assert_eq!(analytics.proposals_created, 1);
            assert_eq!(analytics.votes_cast, 2);
            assert_eq!(analytics.upgrades_executed, 1);
            assert_eq!(analytics.migrations_run, 1);
        });
    }

    #[test]
    fn test_governance_versioned_events() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        client.init_admin(&admin);

        // 1. Test migrate -> emits MigrationCompleted event
        let events_len_before = env.events().all().len();
        let migration_hash = BytesN::from_array(&env, &[9u8; 32]);
        client.migrate(&3, &migration_hash);
        let events = env.events().all();
        assert!(events.len() > events_len_before);

        let mut found_mig_comp = false;
        for event in events.iter() {
            if event.0 == contract_id && event.1.len() > 0 && soroban_sdk::Symbol::try_from_val(&env, &event.1.get(0).unwrap()) == Ok(symbol_short!("mig_comp")) {
                let val: MigrationCompleted = event.2.into_val(&env);
                assert_eq!(val.version, EVENT_VERSION);
                assert_eq!(val.from_version, 2);
                assert_eq!(val.to_version, 3);
                assert_eq!(val.migration_hash, migration_hash);
                assert!(val.success);
                found_mig_comp = true;
            }
        }
        assert!(found_mig_comp);

        // 2. Test set_version -> emits VersionChanged event
        let events_len_before = env.events().all().len();
        client.set_version(&4);
        let events = env.events().all();
        assert!(events.len() > events_len_before);
        
        // Find VersionChanged event
        let mut found_ver_chg = false;
        for event in events.iter() {
            if event.0 == contract_id && event.1.len() > 0 && soroban_sdk::Symbol::try_from_val(&env, &event.1.get(0).unwrap()) == Ok(symbol_short!("ver_chg")) {
                let val: VersionChanged = event.2.into_val(&env);
                assert_eq!(val.version, EVENT_VERSION);
                assert_eq!(val.old_version, 3);
                assert_eq!(val.new_version, 4);
                assert_eq!(val.admin, admin);
                found_ver_chg = true;
            }
        }
        assert!(found_ver_chg);

        // 3. Test single-admin upgrade -> emits UpgradeExecuted
        let wasm_hash = env.deployer().upload_contract_wasm([].as_slice());
        client.set_upgrade_delay(&600);
        client.schedule_upgrade(&wasm_hash);

        env.ledger().with_mut(|li| li.timestamp = 600);
        let events_len_before = env.events().all().len();
        client.upgrade(&wasm_hash);
        let events = env.events().all();
        assert!(events.len() > events_len_before);

        let mut found_upg_exec = false;
        for event in events.iter() {
            if event.0 == contract_id && event.1.len() > 0 && soroban_sdk::Symbol::try_from_val(&env, &event.1.get(0).unwrap()) == Ok(symbol_short!("upg_exec2")) {
                let val: UpgradeExecuted = event.2.into_val(&env);
                assert_eq!(val.version, EVENT_VERSION);
                assert!(val.proposal_id.is_none());
                assert_eq!(val.wasm_hash, wasm_hash);
                found_upg_exec = true;
            }
        }
        assert!(found_upg_exec);
    }

    #[test]
    fn test_multisig_upgrade_events() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let client = GrainlifyContractClient::new(&env, &contract_id);

        let mut signers = soroban_sdk::Vec::new(&env);
        let signer_a = Address::generate(&env);
        let signer_b = Address::generate(&env);
        signers.push_back(signer_a.clone());
        signers.push_back(signer_b.clone());

        // Initialize multisig with threshold 2
        client.init(&signers, &2);

        let wasm_hash = env.deployer().upload_contract_wasm([].as_slice());

        // 1. Propose -> emits UpgradeProposed
        let events_len_before = env.events().all().len();
        let proposal_id = client.propose_upgrade(&signer_a, &wasm_hash);
        let events = env.events().all();
        assert!(events.len() > events_len_before);

        let mut found_upg_prop = false;
        for event in events.iter() {
            if event.0 == contract_id && event.1.len() > 0 && soroban_sdk::Symbol::try_from_val(&env, &event.1.get(0).unwrap()) == Ok(symbol_short!("upg_prop")) {
                let val: UpgradeProposed = event.2.into_val(&env);
                assert_eq!(val.version, EVENT_VERSION);
                assert_eq!(val.proposal_id, proposal_id);
                assert_eq!(val.proposer, signer_a);
                assert_eq!(val.wasm_hash, wasm_hash);
                found_upg_prop = true;
            }
        }
        assert!(found_upg_prop);

        // 2. Approve -> emits UpgradeApproved
        let events_len_before = env.events().all().len();
        client.approve_upgrade(&proposal_id, &signer_a);
        let events = env.events().all();
        assert!(events.len() > events_len_before);

        let mut found_upg_appr = false;
        for event in events.iter() {
            if event.0 == contract_id && event.1.len() > 0 && soroban_sdk::Symbol::try_from_val(&env, &event.1.get(0).unwrap()) == Ok(symbol_short!("upg_appr")) {
                let val: UpgradeApproved = event.2.into_val(&env);
                assert_eq!(val.version, EVENT_VERSION);
                assert_eq!(val.proposal_id, proposal_id);
                assert_eq!(val.signer, signer_a);
                assert_eq!(val.approval_count, 1);
                found_upg_appr = true;
            }
        }
        assert!(found_upg_appr);

        // Approve second signer
        client.approve_upgrade(&proposal_id, &signer_b);

        // 3. Execute -> emits UpgradeExecuted
        let events_len_before = env.events().all().len();
        assert_eq!(client.multisig_nonce(), 0);
        client.execute_upgrade(&proposal_id, &0u64);
        assert_eq!(client.multisig_nonce(), 1);
        let events = env.events().all();
        assert!(events.len() > events_len_before);

        let mut found_upg_exec = false;
        for event in events.iter() {
            if event.0 == contract_id && event.1.len() > 0 && soroban_sdk::Symbol::try_from_val(&env, &event.1.get(0).unwrap()) == Ok(symbol_short!("upg_exec2")) {
                let val: UpgradeExecuted = event.2.into_val(&env);
                assert_eq!(val.version, EVENT_VERSION);
                assert_eq!(val.proposal_id, Some(proposal_id));
                assert_eq!(val.wasm_hash, wasm_hash);
                found_upg_exec = true;
            }
        }
        assert!(found_upg_exec);
    }
}
