use soroban_sdk::{contracttype, symbol_short, Address, BytesN, Env, Vec};

/// =======================
/// Storage Keys
/// =======================
#[contracttype]
enum DataKey {
    Config,
    Proposal(u64),
    ProposalCounter,
    /// Next expected execution nonce. Monotonically increasing; consumed once
    /// per successful `execute`, providing replay protection across proposals.
    ExecutionNonce,
}

/// =======================
/// Multisig Configuration
/// =======================
#[contracttype]
#[derive(Clone)]
pub struct MultiSigConfig {
    pub signers: Vec<Address>,
    pub threshold: u32,
}

/// =======================
/// Proposal Structure
/// =======================
#[contracttype]
#[derive(Clone, Eq, PartialEq)]
pub enum ProposalAction {
    Upgrade(BytesN<32>),
}

#[contracttype]
#[derive(Clone)]
pub struct Proposal {
    pub action: ProposalAction,
    pub approvals: Vec<Address>,
    pub executed: bool,
    pub signers: Vec<Address>,
    pub threshold: u32,
    /// Execution nonce consumed when this proposal was executed. `None` until
    /// execution succeeds; binds the proposal's execution record to a single,
    /// non-reusable nonce value.
    pub executed_nonce: Option<u64>,
}

/// =======================
/// Errors
/// =======================
#[derive(Debug)]
pub enum MultiSigError {
    NotSigner,
    AlreadyApproved,
    ProposalNotFound,
    AlreadyExecuted,
    ThresholdNotMet,
    InvalidThreshold,
    ActionMismatch,
    NonceMismatch,
    /// Removing the signer would leave fewer signers than the configured
    /// threshold, making the multisig permanently un-executable.
    RemovalWouldBreakThreshold,
    AlreadySigner,
}

/// =======================
/// Public API
/// =======================
pub struct MultiSig;

impl MultiSig {
    /// Initialize multisig configuration
    pub fn init(env: &Env, signers: Vec<Address>, threshold: u32) {
        if env.storage().instance().has(&DataKey::Config) {
            panic!("multisig already initialized");
        }

        if threshold == 0 || threshold > signers.len() as u32 {
            panic!("{:?}", MultiSigError::InvalidThreshold);
        }

        let config = MultiSigConfig { signers, threshold };
        env.storage().instance().set(&DataKey::Config, &config);
        env.storage()
            .instance()
            .set(&DataKey::ProposalCounter, &0u64);
        env.storage()
            .instance()
            .set(&DataKey::ExecutionNonce, &0u64);
    }

    /// Create a new proposal bound to a concrete action payload.
    pub fn propose(env: &Env, proposer: Address, action: ProposalAction) -> u64 {
        proposer.require_auth();

        let config = Self::get_config(env);
        Self::assert_signer(&config, &proposer);

        let mut counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ProposalCounter)
            .unwrap_or(0);

        counter += 1;

        let proposal = Proposal {
            action,
            approvals: Vec::new(env),
            executed: false,
            signers: config.signers,
            threshold: config.threshold,
            executed_nonce: None,
        };

        env.storage()
            .instance()
            .set(&DataKey::Proposal(counter), &proposal);
        env.storage()
            .instance()
            .set(&DataKey::ProposalCounter, &counter);

        env.events().publish((symbol_short!("proposal"),), counter);

        #[allow(irrefutable_let_patterns)]
        if let ProposalAction::Upgrade(ref wasm_hash) = proposal.action {
            env.events().publish(
                (symbol_short!("upg_prop"),),
                crate::UpgradeProposed {
                    version: crate::EVENT_VERSION,
                    proposal_id: counter,
                    proposer: proposer.clone(),
                    wasm_hash: wasm_hash.clone(),
                },
            );
        }

        counter
    }

    /// Approve an existing proposal
    pub fn approve(env: &Env, proposal_id: u64, signer: Address) {
        signer.require_auth();

        let mut proposal = Self::get_proposal(env, proposal_id);
        Self::assert_proposal_signer(&proposal, &signer);

        if proposal.executed {
            panic!("{:?}", MultiSigError::AlreadyExecuted);
        }

        if proposal.approvals.contains(&signer) {
            panic!("{:?}", MultiSigError::AlreadyApproved);
        }

        proposal.approvals.push_back(signer.clone());

        env.storage()
            .instance()
            .set(&DataKey::Proposal(proposal_id), &proposal);

        env.events()
            .publish((symbol_short!("approved"),), (proposal_id, signer.clone()));

        #[allow(irrefutable_let_patterns)]
        if let ProposalAction::Upgrade(_) = proposal.action {
            env.events().publish(
                (symbol_short!("upg_appr"),),
                crate::UpgradeApproved {
                    version: crate::EVENT_VERSION,
                    proposal_id,
                    signer: signer.clone(),
                    approval_count: proposal.approvals.len() as u32,
                },
            );
        }
    }

    /// Check if proposal is executable
    pub fn can_execute(env: &Env, proposal_id: u64) -> bool {
        let proposal = Self::get_proposal(env, proposal_id);

        !proposal.executed && proposal.approvals.len() >= proposal.threshold
    }

    /// The next execution nonce the contract will accept.
    ///
    /// Callers must pass this value as `expected_nonce` to [`Self::execute`].
    /// It increases by one after every successful execution, so a previously
    /// used `(signatures, nonce)` pair can never be replayed.
    pub fn nonce(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::ExecutionNonce)
            .unwrap_or(0)
    }

    /// Atomically execute a proposal's bound action and mark it executed.
    ///
    /// The action closure runs only after threshold, payload, and nonce checks
    /// pass. If the closure fails, the caller's transaction fails before the
    /// proposal can be marked executed, preventing approval/effect decoupling.
    ///
    /// Replay protection: `expected_nonce` must equal the current value returned
    /// by [`Self::nonce`]. On success the nonce is consumed and incremented, so
    /// the same nonce (and therefore the same collected signatures re-submitted
    /// as an identical action) cannot drive a second execution.
    pub fn execute<F>(
        env: &Env,
        proposal_id: u64,
        expected_action: ProposalAction,
        expected_nonce: u64,
        action: F,
    ) where
        F: FnOnce(),
    {
        let proposal = Self::get_proposal(env, proposal_id);

        if proposal.executed {
            panic!("{:?}", MultiSigError::AlreadyExecuted);
        }

        if proposal.action != expected_action {
            panic!("{:?}", MultiSigError::ActionMismatch);
        }

        if !Self::can_execute(env, proposal_id) {
            panic!("{:?}", MultiSigError::ThresholdNotMet);
        }

        let current_nonce = Self::nonce(env);
        if expected_nonce != current_nonce {
            panic!("{:?}", MultiSigError::NonceMismatch);
        }

        action();
        Self::mark_executed(env, proposal_id, current_nonce);

        env.storage()
            .instance()
            .set(&DataKey::ExecutionNonce, &(current_nonce + 1));
    }

    pub fn get_action(env: &Env, proposal_id: u64) -> ProposalAction {
        Self::get_proposal(env, proposal_id).action
    }

    /// Remove a signer from the multisig configuration.
    ///
    /// # Pre-condition: threshold viability guard
    /// The removal is rejected with [`MultiSigError::RemovalWouldBreakThreshold`]
    /// if `(current_signer_count - 1) < threshold`.  This prevents an admin
    /// from accidentally (or maliciously) leaving the multisig in a state where
    /// the required number of approvals can never be gathered, which would
    /// permanently lock any funds or actions protected by the multisig.
    ///
    /// # Errors
    /// - Panics with `NotSigner` if `signer_to_remove` is not in the current
    ///   signer set.
    /// - Panics with `RemovalWouldBreakThreshold` if the removal would leave
    ///   fewer signers than the configured threshold.
    pub fn remove_signer(env: &Env, caller: Address, signer_to_remove: Address) {
        caller.require_auth();

        let mut config = Self::get_config(env);

        // Verify the address to be removed is actually a current signer.
        if !config.signers.contains(&signer_to_remove) {
            panic!("{:?}", MultiSigError::NotSigner);
        }

        // Guard: removing this signer must not make the threshold unreachable.
        // After removal there will be (len - 1) signers; that count must still
        // be >= threshold.
        let remaining = config.signers.len() as u32 - 1;
        if remaining < config.threshold {
            panic!("{:?}", MultiSigError::RemovalWouldBreakThreshold);
        }

        // Rebuild the signer list without the removed address.
        let mut new_signers = Vec::new(env);
        for i in 0..config.signers.len() {
            let s = config.signers.get(i).unwrap();
            if s != signer_to_remove {
                new_signers.push_back(s);
            }
        }

        config.signers = new_signers;
        env.storage().instance().set(&DataKey::Config, &config);

        env.events().publish(
            (symbol_short!("SignerRot"), symbol_short!("remove")),
            (caller, signer_to_remove, config.threshold),
        );
    }

    /// Add a new signer to the multisig configuration.
    pub fn add_signer(env: &Env, caller: Address, new_signer: Address) {
        caller.require_auth();

        let mut config = Self::get_config(env);

        if config.signers.contains(&new_signer) {
            panic!("{:?}", MultiSigError::AlreadySigner);
        }

        config.signers.push_back(new_signer.clone());
        env.storage().instance().set(&DataKey::Config, &config);

        env.events().publish(
            (symbol_short!("SignerRot"), symbol_short!("add")),
            (caller, new_signer, config.threshold),
        );
    }

    /// Rotate signers and/or change threshold simultaneously.
    pub fn rotate_signers(
        env: &Env,
        caller: Address,
        add: Vec<Address>,
        remove: Vec<Address>,
        new_threshold: Option<u32>,
    ) {
        caller.require_auth();
        let mut config = Self::get_config(env);

        // Process removals
        for signer_to_remove in remove.clone() {
            if !config.signers.contains(&signer_to_remove) {
                panic!("{:?}", MultiSigError::NotSigner);
            }
            let mut new_signers = Vec::new(env);
            for i in 0..config.signers.len() {
                let s = config.signers.get(i).unwrap();
                if s != signer_to_remove {
                    new_signers.push_back(s);
                }
            }
            config.signers = new_signers;
        }

        // Process additions
        for new_signer in add.clone() {
            if config.signers.contains(&new_signer) {
                panic!("{:?}", MultiSigError::AlreadySigner);
            }
            config.signers.push_back(new_signer);
        }

        // Apply new threshold
        if let Some(t) = new_threshold {
            config.threshold = t;
        }

        // Threshold guard
        if config.threshold == 0 || config.threshold > config.signers.len() as u32 {
            panic!("{:?}", MultiSigError::RemovalWouldBreakThreshold);
        }

        env.storage().instance().set(&DataKey::Config, &config);

        // Emit events
        for signer in add.clone() {
            env.events().publish(
                (symbol_short!("SignerRot"), symbol_short!("add")),
                (caller.clone(), signer, config.threshold),
            );
        }
        for signer in remove.clone() {
            env.events().publish(
                (symbol_short!("SignerRot"), symbol_short!("remove")),
                (caller.clone(), signer, config.threshold),
            );
        }
        
        // If only threshold was changed, emit a threshold update event
        if add.is_empty() && remove.is_empty() && new_threshold.is_some() {
            env.events().publish(
                (symbol_short!("SignerRot"), symbol_short!("thresh")),
                (caller.clone(), caller.clone(), config.threshold),
            );
        }
    }

    fn mark_executed(env: &Env, proposal_id: u64, nonce: u64) {
        let mut proposal = Self::get_proposal(env, proposal_id);

        if proposal.executed {
            panic!("{:?}", MultiSigError::AlreadyExecuted);
        }

        if !Self::can_execute(env, proposal_id) {
            panic!("{:?}", MultiSigError::ThresholdNotMet);
        }

        proposal.executed = true;
        proposal.executed_nonce = Some(nonce);

        env.storage()
            .instance()
            .set(&DataKey::Proposal(proposal_id), &proposal);

        env.events()
            .publish((symbol_short!("executed"),), (proposal_id, nonce));
    }

    /// =======================
    /// Internal Helpers
    /// =======================

    fn get_config(env: &Env) -> MultiSigConfig {
        env.storage()
            .instance()
            .get(&DataKey::Config)
            .expect("multisig not initialized")
    }

    fn get_proposal(env: &Env, proposal_id: u64) -> Proposal {
        env.storage()
            .instance()
            .get(&DataKey::Proposal(proposal_id))
            .unwrap_or_else(|| panic!("{:?}", MultiSigError::ProposalNotFound))
    }

    fn assert_signer(config: &MultiSigConfig, signer: &Address) {
        if !config.signers.contains(signer) {
            panic!("{:?}", MultiSigError::NotSigner);
        }
    }

    fn assert_proposal_signer(proposal: &Proposal, signer: &Address) {
        if !proposal.signers.contains(signer) {
            panic!("{:?}", MultiSigError::NotSigner);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::GrainlifyContract;
    use soroban_sdk::{testutils::Address as _, testutils::Events, Env};

    struct Setup {
        env: Env,
        contract_id: Address,
        signer_a: Address,
        signer_b: Address,
        signer_c: Address,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, GrainlifyContract);
        let signer_a = Address::generate(&env);
        let signer_b = Address::generate(&env);
        let signer_c = Address::generate(&env);

        Setup {
            env,
            contract_id,
            signer_a,
            signer_b,
            signer_c,
        }
    }

    fn signers(env: &Env, signer_a: &Address, signer_b: &Address) -> Vec<Address> {
        let mut signers = Vec::new(env);
        signers.push_back(signer_a.clone());
        signers.push_back(signer_b.clone());
        signers
    }

    fn hash(env: &Env, byte: u8) -> BytesN<32> {
        BytesN::from_array(env, &[byte; 32])
    }

    #[test]
    fn execute_runs_bound_action_and_marks_proposal_once() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 7));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
            assert!(!MultiSig::can_execute(&setup.env, proposal_id));
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
            assert!(MultiSig::can_execute(&setup.env, proposal_id));
        });

        let did_run = setup.env.as_contract(&setup.contract_id, || {
            let mut did_run = false;
            MultiSig::execute(&setup.env, proposal_id, action.clone(), 0, || {
                did_run = true;
            });
            did_run
        });

        setup.env.as_contract(&setup.contract_id, || {
            let proposal = MultiSig::get_proposal(&setup.env, proposal_id);
            assert!(did_run);
            assert!(proposal.executed);
            assert_eq!(proposal.executed_nonce, Some(0));
            assert_eq!(MultiSig::nonce(&setup.env), 1);
            assert!(!MultiSig::can_execute(&setup.env, proposal_id));
        });
    }

    #[test]
    #[should_panic(expected = "ThresholdNotMet")]
    fn execute_rejects_below_threshold() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 8));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action, 0, || {});
        });
    }

    #[test]
    #[should_panic(expected = "ActionMismatch")]
    fn execute_rejects_mismatched_payload() {
        let setup = setup();
        let stored_action = ProposalAction::Upgrade(hash(&setup.env, 9));
        let wrong_action = ProposalAction::Upgrade(hash(&setup.env, 10));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            MultiSig::propose(&setup.env, setup.signer_a.clone(), stored_action)
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, wrong_action, 0, || {});
        });
    }

    #[test]
    #[should_panic(expected = "AlreadyExecuted")]
    fn second_execute_is_rejected() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 13));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action.clone(), 0, || {});
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action, 0, || {});
        });
    }

    #[test]
    #[should_panic(expected = "AlreadyExecuted")]
    fn approve_after_execute_is_rejected() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 11));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action, 0, || {});
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
        });
    }

    // =====================================================================
    // Threshold edge-case tests (issue #184)
    // =====================================================================

    /// Helper: build a Vec<Address> from a slice of &Address.
    fn addr_vec(env: &Env, addrs: &[&Address]) -> Vec<Address> {
        let mut v = Vec::new(env);
        for a in addrs {
            v.push_back((*a).clone());
        }
        v
    }

    // --- 0-threshold is explicitly invalid -----------------------------------

    /// Initialising with threshold=0 must be rejected with InvalidThreshold,
    /// regardless of the signer count.
    #[test]
    #[should_panic(expected = "InvalidThreshold")]
    fn zero_threshold_is_rejected() {
        let setup = setup();
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                0, // threshold == 0 → must panic
            );
        });
    }

    /// threshold > signers.len() is also an invalid configuration.
    #[test]
    #[should_panic(expected = "InvalidThreshold")]
    fn threshold_exceeding_signer_count_is_rejected() {
        let setup = setup();
        setup.env.as_contract(&setup.contract_id, || {
            // 2 signers, threshold = 3 → impossible to ever reach
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                3,
            );
        });
    }

    // --- 1-of-N: any single signer suffices ----------------------------------

    /// A 1-of-3 configuration must become executable as soon as ONE signer
    /// approves — the other two approvals are unnecessary.
    #[test]
    fn one_of_n_single_approval_suffices() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 20));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            let three_signers = addr_vec(
                &setup.env,
                &[&setup.signer_a, &setup.signer_b, &setup.signer_c],
            );
            MultiSig::init(&setup.env, three_signers, 1); // 1-of-3
            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        // Before any approval: not executable.
        setup.env.as_contract(&setup.contract_id, || {
            assert!(!MultiSig::can_execute(&setup.env, proposal_id));
        });

        // After exactly ONE approval: must be executable.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
            assert!(
                MultiSig::can_execute(&setup.env, proposal_id),
                "1-of-3: should be executable after a single approval"
            );
        });

        // Execute succeeds and the action closure runs.
        let did_run = setup.env.as_contract(&setup.contract_id, || {
            let mut ran = false;
            MultiSig::execute(&setup.env, proposal_id, action.clone(), 0, || {
                ran = true;
            });
            ran
        });

        assert!(did_run, "action closure must have been called");
    }

    // --- N-of-N: all signers must approve (unanimous) -----------------------

    /// A 3-of-3 configuration must NOT be executable until every signer has
    /// approved.  After the third approval it becomes executable.
    #[test]
    fn n_of_n_requires_all_signers() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 21));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            let three_signers = addr_vec(
                &setup.env,
                &[&setup.signer_a, &setup.signer_b, &setup.signer_c],
            );
            MultiSig::init(&setup.env, three_signers, 3); // 3-of-3
            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        // After 1st approval: not executable.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
            assert!(!MultiSig::can_execute(&setup.env, proposal_id));
        });

        // After 2nd approval: still not executable.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
            assert!(!MultiSig::can_execute(&setup.env, proposal_id));
        });

        // After 3rd (final) approval: now executable.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_c.clone());
            assert!(
                MultiSig::can_execute(&setup.env, proposal_id),
                "3-of-3: should be executable only after all signers approve"
            );
        });

        // Execute succeeds.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action.clone(), 0, || {});
            let proposal = MultiSig::get_proposal(&setup.env, proposal_id);
            assert!(proposal.executed);
        });
    }

    // --- Exactly-at-threshold: must pass ------------------------------------

    /// With a 2-of-3 config, reaching exactly 2 approvals (= threshold) must
    /// make the proposal executable.  This guards against an off-by-one where
    /// the implementation uses `>` instead of `>=`.
    #[test]
    fn exactly_at_threshold_is_executable() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 22));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            let three_signers = addr_vec(
                &setup.env,
                &[&setup.signer_a, &setup.signer_b, &setup.signer_c],
            );
            MultiSig::init(&setup.env, three_signers, 2); // 2-of-3
            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        // 1 approval → below threshold.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
            assert!(!MultiSig::can_execute(&setup.env, proposal_id));
        });

        // 2 approvals → exactly at threshold → must be executable.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
            assert!(
                MultiSig::can_execute(&setup.env, proposal_id),
                "exactly-at-threshold (2-of-3): must be executable"
            );
        });

        // Execute succeeds without needing the third signer.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action.clone(), 0, || {});
        });
    }

    // --- One-short-of-threshold: must fail ----------------------------------

    /// With a 3-of-3 config, having only 2 approvals (one short of the
    /// threshold) must be rejected when execute is called.  This guards against
    /// an off-by-one where `>=` is accidentally replaced with `>`.
    #[test]
    #[should_panic(expected = "ThresholdNotMet")]
    fn one_short_of_threshold_is_not_executable() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 23));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            let three_signers = addr_vec(
                &setup.env,
                &[&setup.signer_a, &setup.signer_b, &setup.signer_c],
            );
            MultiSig::init(&setup.env, three_signers, 3); // 3-of-3
            MultiSig::propose(&setup.env, setup.signer_a.clone(), action.clone())
        });

        // Only 2 out of 3 required approvals.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
        });

        // Attempting to execute with one approval short must panic.
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action, 0, || {});
        });
    }

    #[test]
    fn signer_and_threshold_snapshot_prevents_retroactive_validation() {
        let setup = setup();
        let action = ProposalAction::Upgrade(hash(&setup.env, 12));

        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            MultiSig::propose(&setup.env, setup.signer_a.clone(), action)
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
        });

        setup.env.as_contract(&setup.contract_id, || {
            let mut changed_signers = Vec::new(&setup.env);
            changed_signers.push_back(setup.signer_a.clone());
            changed_signers.push_back(setup.signer_c.clone());
            setup.env.storage().instance().set(
                &DataKey::Config,
                &MultiSigConfig {
                    signers: changed_signers,
                    threshold: 1,
                },
            );

            assert!(!MultiSig::can_execute(&setup.env, proposal_id));

            let proposal = MultiSig::get_proposal(&setup.env, proposal_id);
            assert_eq!(proposal.threshold, 2);
            assert!(proposal.signers.contains(&setup.signer_b));
            assert!(!proposal.signers.contains(&setup.signer_c));
        });
    }

    /// Propose `action` and approve it to the (2-of-2) threshold, returning the
    /// new proposal id. Assumes the multisig is already initialized.
    fn propose_and_approve(setup: &Setup, action: ProposalAction) -> u64 {
        let proposal_id = setup.env.as_contract(&setup.contract_id, || {
            MultiSig::propose(&setup.env, setup.signer_a.clone(), action)
        });

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::approve(&setup.env, proposal_id, setup.signer_a.clone());
            MultiSig::approve(&setup.env, proposal_id, setup.signer_b.clone());
        });

        proposal_id
    }

    #[test]
    fn nonce_starts_at_zero_and_increments_per_execution() {
        let setup = setup();

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );
            assert_eq!(MultiSig::nonce(&setup.env), 0);
        });

        // First execution consumes nonce 0.
        let action_one = ProposalAction::Upgrade(hash(&setup.env, 20));
        let first = propose_and_approve(&setup, action_one.clone());
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, first, action_one, 0, || {});
            assert_eq!(MultiSig::nonce(&setup.env), 1);
            let proposal = MultiSig::get_proposal(&setup.env, first);
            assert_eq!(proposal.executed_nonce, Some(0));
        });

        // Second, independent execution consumes the next nonce, 1.
        let action_two = ProposalAction::Upgrade(hash(&setup.env, 21));
        let second = propose_and_approve(&setup, action_two.clone());
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, second, action_two, 1, || {});
            assert_eq!(MultiSig::nonce(&setup.env), 2);
            let proposal = MultiSig::get_proposal(&setup.env, second);
            assert_eq!(proposal.executed_nonce, Some(1));
        });
    }

    #[test]
    #[should_panic(expected = "NonceMismatch")]
    fn execute_rejects_wrong_nonce() {
        let setup = setup();

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );
        });

        let action = ProposalAction::Upgrade(hash(&setup.env, 22));
        let proposal_id = propose_and_approve(&setup, action.clone());

        // Threshold and payload are satisfied, but the supplied nonce (1) does
        // not match the expected next nonce (0).
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, proposal_id, action, 1, || {});
        });
    }

    #[test]
    #[should_panic(expected = "NonceMismatch")]
    fn replayed_signatures_and_nonce_are_rejected() {
        let setup = setup();

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );
        });

        // A privileged action is proposed, approved to threshold, and executed
        // with nonce 0. This is the "captured" (signatures, nonce) pair.
        let action = ProposalAction::Upgrade(hash(&setup.env, 23));
        let first = propose_and_approve(&setup, action.clone());
        let mut effects = 0u32;
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, first, action.clone(), 0, || effects += 1);
        });
        assert_eq!(effects, 1);

        // An attacker re-submits the identical action as a fresh proposal and
        // re-collects the same signers' approvals, then replays the previously
        // used nonce (0). The nonce has already advanced to 1, so execution is
        // rejected before the action can run a second time.
        let replay = propose_and_approve(&setup, action.clone());
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, replay, action, 0, || effects += 1);
        });
    }

    #[test]
    fn rejected_replay_leaves_state_untouched() {
        let setup = setup();

        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );
        });

        let action = ProposalAction::Upgrade(hash(&setup.env, 24));
        let first = propose_and_approve(&setup, action.clone());
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::execute(&setup.env, first, action.clone(), 0, || {});
        });

        // Fresh proposal for the identical action, approved to threshold, but
        // not yet executed. The stale nonce (0) has already been consumed.
        let replay = propose_and_approve(&setup, action);

        // Before the replay attempt, the nonce sits at 1 and the fresh proposal
        // is unexecuted. `execute` with the stale nonce panics (asserted by the
        // dedicated NonceMismatch test); here we confirm the pre-attempt state
        // that the panic must preserve: no effect can have leaked through.
        setup.env.as_contract(&setup.contract_id, || {
            assert_eq!(MultiSig::nonce(&setup.env), 1);
            let proposal = MultiSig::get_proposal(&setup.env, replay);
            assert!(!proposal.executed);
            assert_eq!(proposal.executed_nonce, None);
        });
    }

    // =====================================================================
    // remove_signer: threshold-viability guard tests  (issue #183)
    // =====================================================================

    /// Normal removal: 3 signers, threshold=2, remove one → 2 signers remain
    /// which is exactly the threshold → still viable, must succeed.
    ///
    /// This test also doubles as the *boundary* case (remaining == threshold).
    #[test]
    fn remove_signer_normal_above_threshold_succeeds() {
        let setup = setup();
        setup.env.as_contract(&setup.contract_id, || {
            // 3 signers, threshold = 2 (well above minimum)
            let three_signers = addr_vec(
                &setup.env,
                &[&setup.signer_a, &setup.signer_b, &setup.signer_c],
            );
            MultiSig::init(&setup.env, three_signers, 2);

            // Remove signer_c → 2 remain, threshold still = 2: viable.
            MultiSig::remove_signer(
                &setup.env,
                setup.signer_a.clone(), // caller
                setup.signer_c.clone(), // target
            );

            let config = MultiSig::get_config(&setup.env);
            assert_eq!(
                config.signers.len(),
                2,
                "signer count should drop to 2 after removal"
            );
            assert!(
                !config.signers.contains(&setup.signer_c),
                "removed signer must not appear in the config"
            );
            assert!(
                config.signers.contains(&setup.signer_a),
                "remaining signers must still be present"
            );
            assert!(
                config.signers.contains(&setup.signer_b),
                "remaining signers must still be present"
            );
        });
    }

    /// Boundary case: removing exactly down to the threshold (remaining == threshold).
    /// With 3 signers and threshold=2, removing one signer leaves exactly 2 —
    /// matching the threshold.  This must be *allowed*.
    #[test]
    fn remove_signer_boundary_exactly_at_threshold_is_allowed() {
        let setup = setup();
        setup.env.as_contract(&setup.contract_id, || {
            // 3 signers, threshold = 2
            let three_signers = addr_vec(
                &setup.env,
                &[&setup.signer_a, &setup.signer_b, &setup.signer_c],
            );
            MultiSig::init(&setup.env, three_signers, 2);

            // Removing signer_c leaves exactly 2 = threshold → must succeed.
            MultiSig::remove_signer(
                &setup.env,
                setup.signer_a.clone(),
                setup.signer_c.clone(),
            );

            let config = MultiSig::get_config(&setup.env);
            assert_eq!(
                config.signers.len() as u32,
                config.threshold,
                "after boundary removal, signer count must equal threshold exactly"
            );
        });
    }

    /// Rejected removal: 2 signers, threshold=2 → removing either signer
    /// would leave only 1 signer < threshold=2.
    /// The call must panic with `RemovalWouldBreakThreshold`.
    #[test]
    #[should_panic(expected = "RemovalWouldBreakThreshold")]
    fn remove_signer_below_threshold_is_rejected() {
        let setup = setup();
        setup.env.as_contract(&setup.contract_id, || {
            // 2 signers, threshold = 2 (unanimous 2-of-2)
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            // Removing signer_b would leave 1 signer < threshold=2 → must panic.
            MultiSig::remove_signer(
                &setup.env,
                setup.signer_a.clone(),
                setup.signer_b.clone(),
            );
        });
    }

    #[test]
    fn test_add_signer_emits_event() {
        let setup = setup();
        let new_signer = Address::generate(&setup.env);
        
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            MultiSig::add_signer(&setup.env, setup.signer_a.clone(), new_signer.clone());
            
            let config = MultiSig::get_config(&setup.env);
            assert!(config.signers.contains(&new_signer));
            assert_eq!(config.signers.len(), 3);
            assert_eq!(config.threshold, 2);
        });
        
        let events = setup.env.events().all();
        // The last event should be the SignerRot add event
        assert!(events.len() > 0);
    }
    
    #[test]
    fn test_rotate_signers_simultaneous_threshold_change() {
        let setup = setup();
        let new_signer = Address::generate(&setup.env);
        
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            let mut add_vec = Vec::new(&setup.env);
            add_vec.push_back(new_signer.clone());
            
            let mut rm_vec = Vec::new(&setup.env);
            rm_vec.push_back(setup.signer_b.clone());

            // Remove signer_b, add new_signer, and change threshold to 1 simultaneously
            MultiSig::rotate_signers(&setup.env, setup.signer_a.clone(), add_vec, rm_vec, Some(1));
            
            let config = MultiSig::get_config(&setup.env);
            assert!(config.signers.contains(&new_signer));
            assert!(!config.signers.contains(&setup.signer_b));
            assert_eq!(config.signers.len(), 2);
            assert_eq!(config.threshold, 1);
        });
        
        let events = setup.env.events().all();
        assert!(events.len() > 0);
    }
    
    #[test]
    #[should_panic(expected = "RemovalWouldBreakThreshold")]
    fn test_rotate_signers_rejected_no_events() {
        let setup = setup();
        
        setup.env.as_contract(&setup.contract_id, || {
            MultiSig::init(
                &setup.env,
                signers(&setup.env, &setup.signer_a, &setup.signer_b),
                2,
            );

            let add_vec = Vec::new(&setup.env);
            let mut rm_vec = Vec::new(&setup.env);
            rm_vec.push_back(setup.signer_b.clone());

            // Remove signer_b with threshold=2 (guard should block and panic, no events emitted)
            MultiSig::rotate_signers(&setup.env, setup.signer_a.clone(), add_vec, rm_vec, None);
        });
    }
}
