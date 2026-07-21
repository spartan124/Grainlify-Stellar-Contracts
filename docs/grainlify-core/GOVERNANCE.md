# Grainlify Governance System

## Overview

The Grainlify governance system enables decentralized decision-making for contract upgrades through proposals, authenticated voting, quorum checks, and approval thresholds. Governance configuration explicitly selects either one-person-one-vote or token-weighted voting.

## Key Parameters

- **Voting Period:** Duration during which votes can be cast.
- **Execution Delay:** Time-lock period after a proposal is approved before it can be executed.
- **Quorum:** Minimum percentage, in basis points, of the scheme-specific total voting power that must participate.
- **Approval Threshold:** Minimum percentage, in basis points, of non-abstaining voting power that must vote `For`.
- **Minimum Proposal Stake:** Minimum balance of the configured governance token required to create a proposal.
- **Governance Token:** Soroban token address used for token-weighted voting and proposal-stake checks.
- **Token Total Voting Power:** Total token voting power used as the denominator for token-weighted quorum. This should match the selected snapshot or stake-lock set.

## Voting Power

### OnePersonOneVote

`OnePersonOneVote` assigns every authenticated voter a constant voting power of `1`. The contract prevents the same address from voting more than once on the same proposal.

Because the contract does not maintain an on-chain voter registry, quorum for this scheme is calculated against `one_person_total_voters` from `GovernanceConfig`. Deployments must keep this value aligned with the eligible electorate.

### TokenWeighted

`TokenWeighted` derives each vote's `voting_power` by reading the voter's balance from the configured governance token contract at vote time:

```text
voting_power = governance_token.balance(voter)
```

The contract rejects votes with zero voting power. Token-weighted quorum is calculated against `GovernanceConfig::token_total_voting_power`, which should represent the total governance-token power eligible at the selected snapshot or stake-lock point.

## Snapshot And Balance Semantics

The standard Soroban token interface exposes current balances, not historical balances. `GovernanceConfig::snapshot_ledger` records the ledger selected by governance policy for a snapshot or stake-lock process, but the contract cannot independently query historical token balances from a normal token contract.

Production token-weighted governance should use one of these mitigations:

- Lock voting stake for the full voting window before proposals can be voted on.
- Use a governance token wrapper that exposes snapshot balances for the configured snapshot ledger.
- Ensure token supply and transferable balances cannot be cheaply manipulated during the voting window.

Without one of these controls, a voter may temporarily acquire tokens, vote, and transfer them away before finalization. The contract mitigates zero-balance voting and uses the configured token address for all reads, but current-balance voting alone does not prevent flash-loan style power inflation.

## Governance Flow

1. **Proposal Creation**
   - The proposer must authorize the call.
   - If `min_proposal_stake > 0`, the proposer must hold at least that much of the configured governance token.
   - Voting starts immediately upon creation.

2. **Voting Period**
   - Eligible voters cast `For`, `Against`, or `Abstain`.
   - The contract derives voting power according to the configured voting scheme.
   - Each address can vote once per proposal.
   - Zero-power votes are rejected.

3. **Finalization**
   - After the voting period ends, anyone can call `finalize_proposal`.
   - Quorum is checked against the scheme-correct total voting power.
   - Approval threshold is checked against `For + Against` voting power, excluding abstentions.
   - If quorum is not met, the proposal is stored as `Rejected`.

4. **Execution**
   - Approved proposals enter the configured execution delay before upgrade execution.
   - Execution logic should preserve the existing time-lock and audit requirements.

## Security Features

- **Authenticated Voting:** `require_auth()` is called for voters and proposers.
- **Double-Voting Prevention:** Each address can vote only once per proposal.
- **Configured Token Reads:** Token-weighted power and stake checks use only `GovernanceConfig::governance_token`.
- **Zero-Power Rejection:** Accounts with no scheme-valid voting power cannot vote.
- **Quorum Enforcement:** Participation is checked before approval threshold math.
- **Time-locked Upgrades:** The execution delay provides a safety buffer for stakeholders to react to approved changes.
- **Minimum Stake Requirement:** Proposal creation can require governance-token ownership, preventing spam proposals by requiring a significant commitment from the proposer.
- **Immutable Logic:** Proposals cannot be modified once created.
- **Action-Bound Multisig Execution:** Multisig upgrade proposals store the exact `ProposalAction::Upgrade(wasm_hash)` that signers approve. The `execute_upgrade` entrypoint replays that stored action in one call and marks the proposal executed only after the WASM update call is made.
- **Signer/Threshold Snapshots:** Each multisig proposal snapshots the signer set and threshold at creation time. Later configuration changes cannot retroactively make a pending proposal executable or authorize a signer that was not part of the original proposal.
- **Replay Protection:** Executed proposals reject further approvals and cannot be executed a second time. In addition, execution is gated by a contract-wide monotonic **execution nonce**: `execute_upgrade` requires the caller to supply the current nonce, which is consumed and incremented on success. A previously used `(approvals, nonce)` pair therefore cannot be replayed — even against a freshly re-proposed, identically-approved action — because the nonce has already advanced.

## Multisig Upgrade Execution

The multisig upgrade path is intentionally payload-bound:

1. `propose_upgrade(proposer, wasm_hash)` creates a multisig proposal whose action is `Upgrade(wasm_hash)`.
2. `approve_upgrade(proposal_id, signer)` records approvals against the proposal's original signer snapshot.
3. `execute_upgrade(proposal_id, expected_nonce)` verifies the proposal is not executed, confirms the stored action is the expected upgrade payload, checks the proposal snapshot threshold, verifies `expected_nonce` equals the current execution nonce, performs `update_current_contract_wasm(wasm_hash)`, then stores `executed = true` (recording the consumed nonce) and increments the execution nonce.

This removes the previous decoupling between approval and effect. A proposal can no longer be marked executed without the approved action being run, and callers cannot execute a different WASM hash than the one signers approved. Callers read the expected nonce with `multisig_nonce()` before submitting; because the nonce is consumed on execution, a leaked set of approvals cannot be replayed to repeat a privileged upgrade.

## Single-Admin Upgrade Timelock

The direct single-admin `upgrade(wasm_hash)` path is a two-step schedule/execute flow. This keeps the emergency admin route available while adding a mandatory observation window before contract code can change.

1. The admin may call `set_upgrade_delay(delay_seconds)` to configure the delay. The contract rejects values below the documented minimum of 300 seconds. If no value is set, the default delay is 86,400 seconds.
2. The admin calls `schedule_upgrade(wasm_hash)`. The contract stores one active scheduled hash with `scheduled_at` and `executable_at = scheduled_at + delay_seconds`.
3. `upgrade(wasm_hash)` can execute only when the active schedule exists, the supplied hash exactly matches the scheduled hash, and the current ledger timestamp is at or after `executable_at`.
4. Early execution is rejected with `Upgrade timelock not elapsed`; hash mismatch is rejected with `Scheduled upgrade hash mismatch`.
5. A later `schedule_upgrade` call replaces the active schedule, so operators can cancel a pending upgrade by scheduling the intended replacement hash and waiting for its delay.

The contract emits `upg_sch` when an upgrade is scheduled and `upg_exec` when the scheduled upgrade executes. Indexers should track these events together with the existing monitoring metrics to audit single-admin upgrade intent and execution.

## Monitoring & Observability

`grainlify-core` is the most security-critical contract in the system because it
controls upgrades and governance. To give operators first-class observability,
the contract maintains persistent metric counters for the key governance and
upgrade operations, mirroring the metric pattern used by the escrow contracts.

### Tracked Counters

| Counter | Storage key | Incremented by |
| --- | --- | --- |
| `proposals_created` | `gov_prop` | `create_proposal` (on success) |
| `votes_cast` | `gov_vote` | `cast_vote` (on success) |
| `upgrades_executed` | `gov_upg` | `upgrade` and `execute_upgrade` (after the WASM update) |
| `migrations_run` | `gov_migr` | `migrate` (once per applied migration) |

All counters use **persistent** storage, so they survive contract WASM upgrades
and accumulate over the full lifetime of the contract. Increments are saturating
and can never wrap or panic.

### Reading Counters

The counters are surfaced through the existing monitoring views:

- `get_analytics()` returns the operation/error metrics plus the four governance
  counters (`proposals_created`, `votes_cast`, `upgrades_executed`,
  `migrations_run`).
- `get_state_snapshot()` returns the same four counters alongside the contract
  state totals, timestamped at the current ledger.

### Metric Events

Every counter increment emits a `GovernanceMetric` event under the topic
`("metric", "gov")` containing:

- `metric` — a short symbol identifying the operation (`proposal`, `vote`,
  `upgrade`, `migrate`).
- `total` — the new running total for that counter.
- `timestamp` — the ledger timestamp of the increment.

Indexers can consume this single, consistent metric stream to chart governance
activity without scanning full transaction history.

### Security Properties

- **Observational only.** Counters are written after an operation has already
  succeeded and are never read to gate authorization or alter control flow. A
  proposal, vote, upgrade, or migration cannot be blocked, allowed, or changed by
  any counter value.
- **Accurate on success only.** `create_proposal` and `cast_vote` increment their
  counters only after the governance module accepts the operation, so rejected
  calls (for example, double votes) are not counted.
- **Single-count migrations.** Idempotent `migrate` re-invocations return before
  the counter is touched, so each applied migration is counted exactly once.

## Versioned Governance & Upgrade Events

To support robust long-term indexer queries and backward-compatible changes, versioned structured events are emitted during the upgrade, governance, version change, and migration lifecycles. All versioned events contain a `version` field (currently set to `1`).

### Event Versioning Constant

- `EVENT_VERSION` = `1` (of type `u32`)

### 1. Upgrade Proposed (`upg_prop`)
Emitted when a multisig contract upgrade proposal is created.
- **Topic**: `("upg_prop",)`
- **Payload (`UpgradeProposed`)**:
  - `version: u32` — Event schema version.
  - `proposal_id: u64` — The unique ID of the multisig proposal.
  - `proposer: Address` — The address of the proposal creator.
  - `wasm_hash: BytesN<32>` — The hash of the proposed WASM binary.

### 2. Upgrade Approved (`upg_appr`)
Emitted when a multisig signer approves a pending upgrade proposal.
- **Topic**: `("upg_appr",)`
- **Payload (`UpgradeApproved`)**:
  - `version: u32` — Event schema version.
  - `proposal_id: u64` — The unique ID of the multisig proposal.
  - `signer: Address` — The address of the signer casting their approval.
  - `approval_count: u32` — The current total number of approvals gathered.

### 3. Upgrade Executed (`upg_exec2`)
Emitted when a scheduled upgrade is executed (both for multisig upgrades and single-admin upgrades).
> [!NOTE]
> Published under the topic `upg_exec2` to prevent breaking existing indexers listening to the unversioned `upg_exec` event.
- **Topic**: `("upg_exec2",)`
- **Payload (`UpgradeExecuted`)**:
  - `version: u32` — Event schema version.
  - `proposal_id: Option<u64>` — `Some(proposal_id)` if executed via multisig, or `None` if executed via the single-admin timelock.
  - `wasm_hash: BytesN<32>` — The hash of the executed WASM binary.

### 4. Version Changed (`ver_chg`)
Emitted when the contract's stored version is set manually by the administrator.
- **Topic**: `("ver_chg",)`
- **Payload (`VersionChanged`)**:
  - `version: u32` — Event schema version.
  - `old_version: u32` — The version number before the change.
  - `new_version: u32` — The version number after the change.
  - `admin: Address` — The administrator address authorizing the version change.

### 5. Migration Completed (`mig_comp`)
Emitted when a migration run finishes (emitted both on success and failure to provide a complete audit trail).
- **Topic**: `("mig_comp",)`
- **Payload (`MigrationCompleted`)**:
  - `version: u32` — Event schema version.
  - `from_version: u32` — The source version migrated from.
  - `to_version: u32` — The target version migrated to.
  - `timestamp: u64` — Ledger timestamp when the migration was processed.
  - `migration_hash: BytesN<32>` — Verification hash for the migration payload.
  - `success: bool` — Whether the migration completed successfully.
  - `error_message: Option<String>` — Error description if the migration failed.

## Cross-Contract Consumption by Escrow Contracts

Both `bounty_escrow` and `program-escrow` integrate with `grainlify-core`
**exclusively through the governance side** of `GrainlifyContract`. Neither
escrow contract calls any multisig entrypoint.

Each escrow crate ships a `governance_integration.rs` module that declares a
cross-contract interface using Soroban's `#[contractclient]` macro:

```rust
#[contractclient(name = "GovernanceClient")]
trait GovernanceInterface {
    fn get_ver(env: Env) -> u32;
    fn is_upg_ok(env: Env, wasm_hash: BytesN<32>) -> bool;
    // bounty_escrow also calls:
    fn get_version_numeric_encoded(env: Env) -> u32;
}
```

### Methods called and their mapping

| Method | `GrainlifyContract` entrypoint | Purpose |
|---|---|---|
| `get_ver()` | `get_ver()` → `get_version()` | Version liveness check. Ensures the governance contract is reachable and meets a configurable `min_governance_version` before an upgrade gate is evaluated. |
| `is_upg_ok(wasm_hash)` | `is_upg_ok(wasm_hash)` → `GovernanceContract::is_upgrade_approved` | Returns `true` only when an `Executed`, post-delay governance proposal for `wasm_hash` exists. Escrow contracts call this to gate any self-upgrade. |
| `get_version_numeric_encoded()` | `get_version_numeric_encoded()` | Used by `bounty_escrow` to enforce a minimum semantic-version gate (`major * 10_000 + minor * 100 + patch`). Not called by `program-escrow`. |

### Integration call path

```text
bounty_escrow / program-escrow
  └── governance_integration::check_upgrade_approval(env, wasm_hash)
          │
          │  cross-contract call via GovernanceClient
          ▼
      GrainlifyContract::is_upg_ok(wasm_hash)
          │
          ▼
      GovernanceContract::is_upgrade_approved(wasm_hash)
          └── scans proposals for Executed status + matching hash + elapsed delay
```

### Architecture boundary

The escrow contracts do **not** need to know whether `grainlify-core` was
configured with governance, multisig, or single-admin upgrade paths. They only
ask "has governance approved this hash?" via `is_upg_ok`. The multisig and
single-admin paths are entirely internal to `grainlify-core`.

This boundary means:

- Escrow contracts are unaffected by changes to multisig signer config.
- `grainlify-core` can be upgraded via any path without breaking escrow
  governance checks, as long as `is_upg_ok` and `get_ver` remain available.
- A misconfigured escrow that calls multisig entrypoints directly would bypass
  the democratic governance record and should be treated as a security defect.

## TODO / Future Enhancements

- [ ] Integrate with a native Soroban token for precise `TokenWeighted` voting power.
- [ ] Implement a dynamic quorum based on historical participation.
- [ ] Add a formal "veto" mechanism for high-stakes upgrades.

---

*Grainlify Governance - Empowering Decentralized Evolution*
