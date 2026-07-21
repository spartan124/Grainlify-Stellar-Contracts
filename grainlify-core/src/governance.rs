use soroban_sdk::{contracttype, symbol_short, token, Address, BytesN, Env, Map, Symbol};

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum ProposalStatus {
    Pending, 
    Active,
    Approved,
    Rejected,
    Executed,
    Expired,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum VoteType {
    For,
    Against,
    Abstain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub enum VotingScheme {
    OnePersonOneVote,
    TokenWeighted,
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct Proposal {
    pub id: u32,
    pub proposer: Address,
    pub new_wasm_hash: BytesN<32>,
    pub description: Symbol,
    pub created_at: u64,
    pub voting_start: u64,
    pub voting_end: u64,
    pub execution_delay: u64,
    pub status: ProposalStatus,
    pub votes_for: i128,
    pub votes_against: i128,
    pub votes_abstain: i128,
    pub total_votes: u32,
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct GovernanceConfig {
    pub voting_period: u64,
    pub execution_delay: u64,
    /// Quorum in basis points against the scheme-specific total voting power.
    pub quorum_percentage: u32,
    /// Approval threshold in basis points against non-abstaining votes.
    pub approval_threshold: u32,
    /// Minimum governance-token balance required to create a proposal.
    pub min_proposal_stake: i128,
    pub voting_scheme: VotingScheme,
    /// Soroban token used for token-weighted votes and proposal stake checks.
    pub governance_token: Address,
    /// Total eligible voters for one-person-one-vote quorum calculations.
    pub one_person_total_voters: u32,
    /// Total token voting power for token-weighted quorum calculations.
    pub token_total_voting_power: i128,
    /// Optional ledger recorded by governance policy for snapshot/stake-lock semantics.
    pub snapshot_ledger: Option<u32>,
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct Vote {
    pub voter: Address,
    pub proposal_id: u32,
    pub vote_type: VoteType,
    pub voting_power: i128,
    pub timestamp: u64,
}

pub const PROPOSALS: Symbol = symbol_short!("PROPOSALS");
pub const PROPOSAL_COUNT: Symbol = symbol_short!("PROP_CNT");
pub const VOTES: Symbol = symbol_short!("VOTES");
pub const GOVERNANCE_CONFIG: Symbol = symbol_short!("GOV_CFG");

#[soroban_sdk::contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    InvalidThreshold = 2,
    ThresholdTooLow = 3,
    InsufficientStake = 4,
    ProposalsNotFound = 5,
    ProposalNotFound = 6,
    ProposalNotActive = 7,
    VotingNotStarted = 8,
    VotingEnded = 9,
    VotingStillActive = 10,
    AlreadyVoted = 11,
    ProposalNotApproved = 12,
    ExecutionDelayNotMet = 13,
    ProposalExpired = 14,
    ZeroVotingPower = 15,
    InvalidTotalVotingPower = 16,
    Unauthorized = 17,
}

#[cfg_attr(not(target_arch = "wasm32"), soroban_sdk::contract)]
pub struct GovernanceContract;

#[cfg_attr(not(target_arch = "wasm32"), soroban_sdk::contractimpl)]
impl GovernanceContract {
    pub fn init_governance(
        env: Env,
        admin: Address,
        config: GovernanceConfig,
    ) -> Result<(), Error> {
        admin.require_auth();
        if config.quorum_percentage > 10000 || config.approval_threshold > 10000 {
            return Err(Error::InvalidThreshold);
        }
        if config.approval_threshold < 5000 {
            return Err(Error::ThresholdTooLow);
        }
        if config.min_proposal_stake < 0 {
            return Err(Error::InsufficientStake);
        }
        if total_voting_power(&config) <= 0 {
            return Err(Error::InvalidTotalVotingPower);
        }

        env.storage().instance().set(&GOVERNANCE_CONFIG, &config);
        env.storage().instance().set(&PROPOSAL_COUNT, &0u32);
        Ok(())
    }

    pub fn create_proposal(
        env: Env,
        proposer: Address,
        new_wasm_hash: BytesN<32>,
        description: Symbol,
    ) -> Result<u32, Error> {
        proposer.require_auth();
        let config: GovernanceConfig = env
            .storage()
            .instance()
            .get(&GOVERNANCE_CONFIG)
            .ok_or(Error::NotInitialized)?;
        enforce_min_proposal_stake(&env, &config, &proposer)?;

        let proposal_id: u32 = env.storage().instance().get(&PROPOSAL_COUNT).unwrap_or(0);
        let current_time = env.ledger().timestamp();

        let proposal = Proposal {
            id: proposal_id,
            proposer: proposer.clone(),
            new_wasm_hash,
            description,
            created_at: current_time,
            voting_start: current_time,
            voting_end: current_time + config.voting_period,
            execution_delay: config.execution_delay,
            status: ProposalStatus::Active,
            votes_for: 0,
            votes_against: 0,
            votes_abstain: 0,
            total_votes: 0,
        };

        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&PROPOSALS)
            .unwrap_or(Map::new(&env));
        proposals.set(proposal_id, proposal);
        env.storage().instance().set(&PROPOSALS, &proposals);
        env.storage()
            .instance()
            .set(&PROPOSAL_COUNT, &(proposal_id + 1));

        Ok(proposal_id)
    }

    pub fn cast_vote(
        env: Env,
        voter: Address,
        proposal_id: u32,
        vote_type: VoteType,
    ) -> Result<(), Error> {
        voter.require_auth();
        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&PROPOSALS)
            .ok_or(Error::ProposalsNotFound)?;
        let mut proposal = proposals.get(proposal_id).ok_or(Error::ProposalNotFound)?;

        if proposal.status != ProposalStatus::Active {
            return Err(Error::ProposalNotActive);
        }

        let current_time = env.ledger().timestamp();
        if current_time > proposal.voting_end {
            return Err(Error::VotingEnded);
        }

        let mut votes: Map<(u32, Address), Vote> = env
            .storage()
            .instance()
            .get(&VOTES)
            .unwrap_or(Map::new(&env));
        if votes.contains_key((proposal_id, voter.clone())) {
            return Err(Error::AlreadyVoted);
        }

        let config: GovernanceConfig = env
            .storage()
            .instance()
            .get(&GOVERNANCE_CONFIG)
            .ok_or(Error::NotInitialized)?;
        let voting_power = derive_voting_power(&env, &config, &voter);
        if voting_power <= 0 {
            return Err(Error::ZeroVotingPower);
        }

        match vote_type {
            VoteType::For => proposal.votes_for += voting_power,
            VoteType::Against => proposal.votes_against += voting_power,
            VoteType::Abstain => proposal.votes_abstain += voting_power,
        }
        proposal.total_votes += 1;

        votes.set(
            (proposal_id, voter.clone()),
            Vote {
                voter,
                proposal_id,
                vote_type,
                voting_power,
                timestamp: current_time,
            },
        );

        proposals.set(proposal_id, proposal);
        env.storage().instance().set(&PROPOSALS, &proposals);
        env.storage().instance().set(&VOTES, &votes);
        Ok(())
    }

    pub fn finalize_proposal(env: Env, proposal_id: u32) -> Result<ProposalStatus, Error> {
        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&PROPOSALS)
            .ok_or(Error::ProposalsNotFound)?;
        let mut proposal = proposals.get(proposal_id).ok_or(Error::ProposalNotFound)?;
        let config: GovernanceConfig = env
            .storage()
            .instance()
            .get(&GOVERNANCE_CONFIG)
            .ok_or(Error::NotInitialized)?;

        if env.ledger().timestamp() <= proposal.voting_end {
            return Err(Error::VotingStillActive);
        }

        let total_cast = proposal.votes_for + proposal.votes_against + proposal.votes_abstain;
        let total_power = total_voting_power(&config);
        if total_power <= 0 {
            return Err(Error::InvalidTotalVotingPower);
        }

        let quorum_bps = (total_cast * 10000) / total_power;
        if quorum_bps < config.quorum_percentage as i128 {
            proposal.status = ProposalStatus::Rejected;
            proposals.set(proposal_id, proposal.clone());
            env.storage().instance().set(&PROPOSALS, &proposals);
            return Ok(proposal.status);
        }

        let approval_votes = proposal.votes_for + proposal.votes_against;
        if approval_votes == 0 {
            proposal.status = ProposalStatus::Rejected;
        } else {
            let approval_bps = (proposal.votes_for * 10000) / approval_votes;
            if approval_bps >= config.approval_threshold as i128 {
                proposal.status = ProposalStatus::Approved;
            } else {
                proposal.status = ProposalStatus::Rejected;
            }
        }

        proposals.set(proposal_id, proposal.clone());
        env.storage().instance().set(&PROPOSALS, &proposals);
        Ok(proposal.status)
    }

    /// Marks an approved proposal as executed after its execution delay has elapsed.
    pub fn execute_proposal(env: Env, proposal_id: u32) -> Result<(), Error> {
        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&PROPOSALS)
            .ok_or(Error::ProposalsNotFound)?;
        let mut proposal = proposals.get(proposal_id).ok_or(Error::ProposalNotFound)?;

        if proposal.status != ProposalStatus::Approved {
            return Err(Error::ProposalNotApproved);
        }

        let executable_at = proposal.voting_end.saturating_add(proposal.execution_delay);
        if env.ledger().timestamp() < executable_at {
            return Err(Error::ExecutionDelayNotMet);
        }

        proposal.status = ProposalStatus::Executed;
        proposals.set(proposal_id, proposal);
        env.storage().instance().set(&PROPOSALS, &proposals);
        Ok(())
    }

    /// Returns true when an executed governance proposal approved `wasm_hash`.
    pub fn is_upgrade_approved(env: Env, wasm_hash: BytesN<32>) -> bool {
        let proposals: Map<u32, Proposal> = match env.storage().instance().get(&PROPOSALS) {
            Some(proposals) => proposals,
            None => return false,
        };
        let proposal_count: u32 = env.storage().instance().get(&PROPOSAL_COUNT).unwrap_or(0);
        let now = env.ledger().timestamp();

        let mut proposal_id = 0;
        while proposal_id < proposal_count {
            if let Some(proposal) = proposals.get(proposal_id) {
                let executable_at = proposal.voting_end.saturating_add(proposal.execution_delay);
                if proposal.new_wasm_hash == wasm_hash
                    && proposal.status == ProposalStatus::Executed
                    && now >= executable_at
                {
                    return true;
                }
            }
            proposal_id += 1;
        }

        false
    }

    /// Get the status of a proposal by ID
    pub fn get_proposal_status(env: Env, proposal_id: u32) -> Result<ProposalStatus, Error> {
        let proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&PROPOSALS)
            .ok_or(Error::ProposalsNotFound)?;
        let proposal = proposals.get(proposal_id).ok_or(Error::ProposalNotFound)?;
        Ok(proposal.status)
    }

    /// Cancel a proposal before it is finalized
    pub fn cancel_proposal(env: Env, caller: Address, proposal_id: u32) -> Result<(), Error> {
        caller.require_auth();

        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&PROPOSALS)
            .ok_or(Error::ProposalsNotFound)?;
        let mut proposal = proposals.get(proposal_id).ok_or(Error::ProposalNotFound)?;

        if proposal.status != ProposalStatus::Active {
            return Err(Error::ProposalNotActive);
        }

        if caller != proposal.proposer {
            return Err(Error::Unauthorized);
        }

        proposal.status = ProposalStatus::Cancelled;
        proposals.set(proposal_id, proposal);
        env.storage().instance().set(&PROPOSALS, &proposals);

        env.events().publish(
            (symbol_short!("PropCanc"), proposal_id),
            caller,
        );

        Ok(())
    }

    /// Sweep expired proposals (those that never reached a final state)
    ///
    /// # Storage Strategy: Flag-Not-Delete
    ///
    /// This function uses a **flag-not-delete** approach for expired proposals.
    /// 
    /// ## Why flag instead of delete?
    /// - **Auditability**: Preserves historical records for governance audits
    /// - **Transparency**: Allows off-chain indexers to see all proposals ever created
    /// - **Accountability**: Provides a complete on-chain record of governance activity
    /// - **Simplicity**: No complex deletion logic or cascading cleanup needed
    ///
    /// ## Soroban Storage TTL Considerations
    /// - Proposal data has a configurable TTL that can be extended via Soroban's
    ///   storage rent mechanism
    /// - The `Expired` flag allows off-chain consumers to filter without recomputing
    ///   expiry logic
    /// - If storage costs become prohibitive in the future, a batch deletion function
    ///   could be added
    ///
    /// ## Permissionless Access
    /// This function is permissionless because it only marks known-expired state
    /// and doesn't affect any active governance processes.
    ///
    /// # Arguments
    /// * `env` - The contract environment
    /// * `proposal_id` - ID of the proposal to sweep
    /// * `current_time` - Current ledger timestamp (passed in for testability)
    ///
    /// # Errors
    /// * `ProposalsNotFound` - No proposals exist in storage
    /// * `ProposalNotFound` - The specified proposal ID doesn't exist
    /// * `ProposalNotActive` - The proposal is not in Active status
    /// * `VotingStillActive` - The voting period hasn't ended yet
    ///
    /// # Example
    /// ```ignore
    /// // Sweep an expired proposal after voting ended
    /// let current_time = env.ledger().timestamp();
    /// client.sweep_expired_proposal(&proposal_id, &current_time);
    /// ```
    pub fn sweep_expired_proposal(env: Env, proposal_id: u32, current_time: u64) -> Result<(), Error> {
        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&PROPOSALS)
            .ok_or(Error::ProposalsNotFound)?;
        let mut proposal = proposals.get(proposal_id).ok_or(Error::ProposalNotFound)?;

        // Only sweep proposals that are still active and past their voting end
        if proposal.status != ProposalStatus::Active {
            return Err(Error::ProposalNotActive);
        }

        if current_time <= proposal.voting_end {
            return Err(Error::VotingStillActive);
        }

        // Mark as expired
        proposal.status = ProposalStatus::Expired;
        proposals.set(proposal_id, proposal);
        env.storage().instance().set(&PROPOSALS, &proposals);
        Ok(())
    }
}

/// Derives voting power for the configured scheme.
///
/// `OnePersonOneVote` always returns `1` for an authenticated address.
/// `TokenWeighted` reads the voter's current balance from the configured
/// governance token. `snapshot_ledger` is recorded policy metadata only; the
/// standard Soroban token interface does not expose historical balances.
fn derive_voting_power(env: &Env, config: &GovernanceConfig, voter: &Address) -> i128 {
    match config.voting_scheme {
        VotingScheme::OnePersonOneVote => 1,
        VotingScheme::TokenWeighted => {
            // SECURITY NOTE: Cross-contract call to the governance token.
            // This is a read-only `balance` query and inherently requires no authorization.
            // The caller's intent is already secured by `voter.require_auth()` in `cast_vote`.
            // There is no over-broad use of `authorize_as_current_contract`.
            token::Client::new(env, &config.governance_token).balance(voter)
        }
    }
}

fn total_voting_power(config: &GovernanceConfig) -> i128 {
    match config.voting_scheme {
        VotingScheme::OnePersonOneVote => config.one_person_total_voters as i128,
        VotingScheme::TokenWeighted => config.token_total_voting_power,
    }
}

fn enforce_min_proposal_stake(
    env: &Env,
    config: &GovernanceConfig,
    proposer: &Address,
) -> Result<(), Error> {
    if config.min_proposal_stake == 0 {
        return Ok(());
    }

    // SECURITY NOTE: Cross-contract call to the governance token.
    // This is a read-only `balance` query and inherently requires no authorization.
    // The proposer's intent is already secured by `proposer.require_auth()` in `create_proposal`.
    // There is no over-broad use of `authorize_as_current_contract`.
    let balance = token::Client::new(env, &config.governance_token).balance(proposer);
    if balance < config.min_proposal_stake {
        return Err(Error::InsufficientStake);
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger};

    fn setup_test(
        env: &Env,
        voting_scheme: VotingScheme,
        quorum_percentage: u32,
        min_proposal_stake: i128,
        one_person_total_voters: u32,
    ) -> (
        GovernanceContractClient<'_>,
        Option<token::StellarAssetClient<'_>>,
        Address,
    ) {
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(env, &contract_id);
        let admin = Address::generate(env);
        let user = Address::generate(env);
        let needs_token = voting_scheme == VotingScheme::TokenWeighted || min_proposal_stake > 0;
        let (token_address, token_admin_client) = if needs_token {
            let token_admin = Address::generate(env);
            let token_address = env
                .register_stellar_asset_contract_v2(token_admin)
                .address();
            (
                token_address.clone(),
                Some(token::StellarAssetClient::new(env, &token_address)),
            )
        } else {
            (Address::generate(env), None)
        };

        let config = GovernanceConfig {
            voting_period: 100,
            execution_delay: 0,
            quorum_percentage,
            approval_threshold: 5000,
            min_proposal_stake,
            voting_scheme,
            governance_token: token_address,
            one_person_total_voters,
            token_total_voting_power: 100,
            snapshot_ledger: None,
        };

        env.mock_all_auths();
        client.init_governance(&admin, &config);
        (client, token_admin_client, user)
    }

    fn create_test_proposal(
        env: &Env,
        client: &GovernanceContractClient,
        proposer: &Address,
    ) -> u32 {
        client.create_proposal(
            proposer,
            &BytesN::from_array(env, &[0u8; 32]),
            &symbol_short!("test"),
        )
    }

    #[test]
    fn test_edge_case_double_voting() {
        let env = Env::default();
        let (client, _, user) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
        let prop_id = create_test_proposal(&env, &client, &user);

        client.cast_vote(&user, &prop_id, &VoteType::For);

        let result = client.try_cast_vote(&user, &prop_id, &VoteType::For);
        assert_eq!(result, Err(Ok(Error::AlreadyVoted)));
    }

    #[test]
    fn test_edge_case_voting_after_expiration() {
        let env = Env::default();
        let (client, _, user) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
        let prop_id = create_test_proposal(&env, &client, &user);

        env.ledger().with_mut(|li| li.timestamp = 200);

        let result = client.try_cast_vote(&user, &prop_id, &VoteType::For);
        assert_eq!(result, Err(Ok(Error::VotingEnded)));
    }

    #[test]
    fn test_edge_case_exact_threshold() {
        let env = Env::default();
        let (client, _, user1) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 2);
        let user2 = Address::generate(&env);
        let prop_id = create_test_proposal(&env, &client, &user1);

        client.cast_vote(&user1, &prop_id, &VoteType::For);
        client.cast_vote(&user2, &prop_id, &VoteType::Against);

        env.ledger().with_mut(|li| li.timestamp = 200);
        let status = client.finalize_proposal(&prop_id);

        assert_eq!(status, ProposalStatus::Approved);
    }

    #[test]
    fn test_edge_case_below_threshold() {
        let env = Env::default();
        let (client, _, user1) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 3);
        let user2 = Address::generate(&env);
        let user3 = Address::generate(&env);
        let prop_id = create_test_proposal(&env, &client, &user1);

        client.cast_vote(&user1, &prop_id, &VoteType::For);
        client.cast_vote(&user2, &prop_id, &VoteType::Against);
        client.cast_vote(&user3, &prop_id, &VoteType::Against);

        env.ledger().with_mut(|li| li.timestamp = 200);
        let status = client.finalize_proposal(&prop_id);

        assert_eq!(status, ProposalStatus::Rejected);
    }

    #[test]
    fn test_token_weighted_derives_power_from_balance() {
        let env = Env::default();
        let (client, token_admin_client, proposer) =
            setup_test(&env, VotingScheme::TokenWeighted, 5000, 0, 0);
        let token_admin_client = token_admin_client.unwrap();
        let voter_for = Address::generate(&env);
        let voter_against = Address::generate(&env);

        token_admin_client.mint(&voter_for, &60);
        token_admin_client.mint(&voter_against, &40);

        let prop_id = create_test_proposal(&env, &client, &proposer);
        client.cast_vote(&voter_for, &prop_id, &VoteType::For);
        client.cast_vote(&voter_against, &prop_id, &VoteType::Against);

        env.ledger().with_mut(|li| li.timestamp = 200);
        let status = client.finalize_proposal(&prop_id);

        assert_eq!(status, ProposalStatus::Approved);
    }

    #[test]
    fn test_token_weighted_rejects_zero_balance_voter() {
        let env = Env::default();
        let (client, _, proposer) = setup_test(&env, VotingScheme::TokenWeighted, 1000, 0, 0);
        let zero_balance_voter = Address::generate(&env);
        let prop_id = create_test_proposal(&env, &client, &proposer);

        let result = client.try_cast_vote(&zero_balance_voter, &prop_id, &VoteType::For);

        assert_eq!(result, Err(Ok(Error::ZeroVotingPower)));
    }

    #[test]
    fn test_token_weighted_quorum_just_met() {
        let env = Env::default();
        let (client, token_admin_client, proposer) =
            setup_test(&env, VotingScheme::TokenWeighted, 5000, 0, 0);
        let token_admin_client = token_admin_client.unwrap();
        let voter = Address::generate(&env);
        let non_voter = Address::generate(&env);

        token_admin_client.mint(&voter, &50);
        token_admin_client.mint(&non_voter, &50);

        let prop_id = create_test_proposal(&env, &client, &proposer);
        client.cast_vote(&voter, &prop_id, &VoteType::For);

        env.ledger().with_mut(|li| li.timestamp = 200);
        assert_eq!(client.finalize_proposal(&prop_id), ProposalStatus::Approved);
    }

    #[test]
    fn test_token_weighted_quorum_just_missed_rejects() {
        let env = Env::default();
        let (client, token_admin_client, proposer) =
            setup_test(&env, VotingScheme::TokenWeighted, 5000, 0, 0);
        let token_admin_client = token_admin_client.unwrap();
        let voter = Address::generate(&env);
        let non_voter = Address::generate(&env);

        token_admin_client.mint(&voter, &49);
        token_admin_client.mint(&non_voter, &51);

        let prop_id = create_test_proposal(&env, &client, &proposer);
        client.cast_vote(&voter, &prop_id, &VoteType::For);

        env.ledger().with_mut(|li| li.timestamp = 200);
        let status = client.finalize_proposal(&prop_id);

        assert_eq!(status, ProposalStatus::Rejected);
    }

    #[test]
    fn test_one_person_and_token_weighted_can_diverge() {
        let env = Env::default();
        let (one_person_client, _, one_person_proposer) =
            setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 2);
        let heavy_against = Address::generate(&env);

        let one_person_prop = create_test_proposal(&env, &one_person_client, &one_person_proposer);
        one_person_client.cast_vote(&one_person_proposer, &one_person_prop, &VoteType::For);
        one_person_client.cast_vote(&heavy_against, &one_person_prop, &VoteType::Against);

        env.ledger().with_mut(|li| li.timestamp = 200);
        assert_eq!(
            one_person_client.finalize_proposal(&one_person_prop),
            ProposalStatus::Approved
        );

        let env = Env::default();
        let (token_client, token_admin_client, token_proposer) =
            setup_test(&env, VotingScheme::TokenWeighted, 1000, 0, 0);
        let token_admin_client = token_admin_client.unwrap();
        let heavy_against = Address::generate(&env);

        token_admin_client.mint(&token_proposer, &1);
        token_admin_client.mint(&heavy_against, &99);

        let token_prop = create_test_proposal(&env, &token_client, &token_proposer);
        token_client.cast_vote(&token_proposer, &token_prop, &VoteType::For);
        token_client.cast_vote(&heavy_against, &token_prop, &VoteType::Against);

        env.ledger().with_mut(|li| li.timestamp = 200);
        assert_eq!(
            token_client.finalize_proposal(&token_prop),
            ProposalStatus::Rejected
        );
    }

    #[test]
    fn test_create_proposal_enforces_minimum_stake() {
        let env = Env::default();
        let (client, token_admin_client, proposer) =
            setup_test(&env, VotingScheme::TokenWeighted, 1000, 10, 0);
        let token_admin_client = token_admin_client.unwrap();

        let result = client.try_create_proposal(
            &proposer,
            &BytesN::from_array(&env, &[0u8; 32]),
            &symbol_short!("test"),
        );
        assert_eq!(result, Err(Ok(Error::InsufficientStake)));

        token_admin_client.mint(&proposer, &10);
        let prop_id = create_test_proposal(&env, &client, &proposer);
        assert_eq!(prop_id, 0);
    }

    #[test]
    fn test_upgrade_approval_requires_executed_matching_hash_after_delay() {
        let env = Env::default();
        let contract_id = env.register_contract(None, GovernanceContract);
        let client = GovernanceContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let proposer = Address::generate(&env);
        let approved_hash = BytesN::from_array(&env, &[7u8; 32]);
        let other_hash = BytesN::from_array(&env, &[9u8; 32]);

        let config = GovernanceConfig {
            voting_period: 100,
            execution_delay: 50,
            quorum_percentage: 1000,
            approval_threshold: 5000,
            min_proposal_stake: 0,
            voting_scheme: VotingScheme::OnePersonOneVote,
            governance_token: Address::generate(&env),
            one_person_total_voters: 1,
            token_total_voting_power: 100,
            snapshot_ledger: None,
        };

        env.mock_all_auths();
        client.init_governance(&admin, &config);
        let proposal_id =
            client.create_proposal(&proposer, &approved_hash, &symbol_short!("upgrade"));
        client.cast_vote(&proposer, &proposal_id, &VoteType::For);

        env.ledger().with_mut(|li| li.timestamp = 101);
        assert_eq!(
            client.finalize_proposal(&proposal_id),
            ProposalStatus::Approved
        );
        assert!(!client.is_upgrade_approved(&approved_hash));
        assert_eq!(
            client.try_execute_proposal(&proposal_id),
            Err(Ok(Error::ExecutionDelayNotMet)),
        );

        env.ledger().with_mut(|li| li.timestamp = 150);
        client.execute_proposal(&proposal_id);

        assert!(client.is_upgrade_approved(&approved_hash));
        assert!(!client.is_upgrade_approved(&other_hash));
    }
#[test]
fn test_sweep_expired_proposal_before_expiry_rejected() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let prop_id = create_test_proposal(&env, &client, &proposer);

    // Proposal voting ends at timestamp 100 (voting_period = 100 from setup_test)
    let current_time = 50; // Before expiry

    let result = client.try_sweep_expired_proposal(&prop_id, &current_time);
    assert_eq!(result, Err(Ok(Error::VotingStillActive)));

    // Verify proposal is still Active - no unwrap needed
    let status = client.get_proposal_status(&prop_id);
    assert_eq!(status, ProposalStatus::Active);
}

#[test]
fn test_sweep_expired_proposal_after_expiry_succeeds() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let prop_id = create_test_proposal(&env, &client, &proposer);

    // Proposal voting ends at timestamp 100 (voting_period = 100 from setup_test)
    let current_time = 150; // After expiry

    let result = client.try_sweep_expired_proposal(&prop_id, &current_time);
    assert!(result.is_ok());

    // Verify proposal status is now Expired - no unwrap needed
    let status = client.get_proposal_status(&prop_id);
    assert_eq!(status, ProposalStatus::Expired);
}

#[test]
fn test_sweep_expired_proposal_nonexistent_fails() {
    let env = Env::default();
    let (client, _, _) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let non_existent_id = 999;

    let result = client.try_sweep_expired_proposal(&non_existent_id, &150);
    assert_eq!(result, Err(Ok(Error::ProposalsNotFound)));  // Changed from ProposalNotFound to ProposalsNotFound
}

#[test]
fn test_sweep_expired_proposal_already_expired_fails() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let prop_id = create_test_proposal(&env, &client, &proposer);

    // First sweep after expiry
    let current_time = 150;
    let result = client.try_sweep_expired_proposal(&prop_id, &current_time);
    assert!(result.is_ok());

    // Verify status is Expired - no unwrap needed
    let status = client.get_proposal_status(&prop_id);
    assert_eq!(status, ProposalStatus::Expired);

    // Try to sweep again - should fail because proposal is no longer Active
    let result2 = client.try_sweep_expired_proposal(&prop_id, &200);
    assert_eq!(result2, Err(Ok(Error::ProposalNotActive)));
}

#[test]
fn test_sweep_expired_proposal_already_finalized_fails() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 2);
    let prop_id = create_test_proposal(&env, &client, &proposer);

    // Vote and finalize the proposal
    let voter2 = Address::generate(&env);
    client.cast_vote(&proposer, &prop_id, &VoteType::For);
    client.cast_vote(&voter2, &prop_id, &VoteType::Against);

    env.ledger().with_mut(|li| li.timestamp = 200);
    let status = client.finalize_proposal(&prop_id);
    assert_eq!(status, ProposalStatus::Approved);

    // Try to sweep - should fail because proposal is not Active
    let result = client.try_sweep_expired_proposal(&prop_id, &200);
    assert_eq!(result, Err(Ok(Error::ProposalNotActive)));
}

#[test]
fn test_cancel_proposal_success() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let prop_id = create_test_proposal(&env, &client, &proposer);

    let result = client.try_cancel_proposal(&proposer, &prop_id);
    assert!(result.is_ok());

    let status = client.get_proposal_status(&prop_id);
    assert_eq!(status, ProposalStatus::Cancelled);
}

#[test]
fn test_cancel_proposal_unauthorized() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let prop_id = create_test_proposal(&env, &client, &proposer);
    let unauthorized = Address::generate(&env);

    let result = client.try_cancel_proposal(&unauthorized, &prop_id);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));

    let status = client.get_proposal_status(&prop_id);
    assert_eq!(status, ProposalStatus::Active);
}

#[test]
fn test_cancel_proposal_after_passing_fails() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let prop_id = create_test_proposal(&env, &client, &proposer);

    client.cast_vote(&proposer, &prop_id, &VoteType::For);
    env.ledger().with_mut(|li| li.timestamp = 200);
    let status = client.finalize_proposal(&prop_id);
    assert_eq!(status, ProposalStatus::Approved);

    let result = client.try_cancel_proposal(&proposer, &prop_id);
    assert_eq!(result, Err(Ok(Error::ProposalNotActive)));
}

#[test]
fn test_cancel_proposal_already_cancelled_fails() {
    let env = Env::default();
    let (client, _, proposer) = setup_test(&env, VotingScheme::OnePersonOneVote, 1000, 0, 10);
    let prop_id = create_test_proposal(&env, &client, &proposer);

    client.cancel_proposal(&proposer, &prop_id);
    
    let result = client.try_cancel_proposal(&proposer, &prop_id);
    assert_eq!(result, Err(Ok(Error::ProposalNotActive)));
}

#[test]
fn test_cross_contract_auth_scope_minimal() {
    let env = Env::default();
    let (client, token_admin_client, proposer) = setup_test(&env, VotingScheme::TokenWeighted, 1000, 10, 0);
    let token_admin_client = token_admin_client.unwrap();

    token_admin_client.mint(&proposer, &50);

    env.mock_all_auths();

    // This will call `enforce_min_proposal_stake` which does a cross-contract call
    let prop_id = create_test_proposal(&env, &client, &proposer);

    // Verify that only the `proposer.require_auth()` was used
    let auths = env.auths();

    // We expect `proposer` to have authorized `create_proposal`.
    // If the contract had broadly used `authorize_as_current_contract`, we would see
    // an authorization from the contract itself for the token balance call.
    let mut contract_authorized = false;
    let contract_id = client.address.clone();
    for (addr, _) in auths.iter() {
        if addr == &contract_id {
            contract_authorized = true;
        }
    }
    assert!(!contract_authorized, "Contract should not broadly authorize cross-contract read calls");

    // Now test cast_vote which calls `derive_voting_power` (also a cross-contract call)
    env.mock_all_auths();
    let voter = Address::generate(&env);
    token_admin_client.mint(&voter, &100);
    client.cast_vote(&voter, &prop_id, &VoteType::For);

    let mut vote_contract_authorized = false;
    for (addr, _) in env.auths().iter() {
        if addr == &contract_id {
            vote_contract_authorized = true;
        }
    }
    assert!(!vote_contract_authorized, "Contract should not broadly authorize cross-contract read calls in cast_vote");
}
}
