use soroban_sdk::{contracttype, symbol_short, Address, Env, String, Symbol};

// Storage keys
const OPERATION_COUNT: &str = "op_count"; // Lifetime metric (never decays)
const USER_COUNT: &str = "usr_count"; // Lifetime metric (never decays)
const ERROR_COUNT: &str = "err_count"; // Lifetime metric (never decays)

// Window keys for metric decay
const WINDOW_START: &str = "win_start";
const WINDOW_OPS: &str = "win_ops";
const WINDOW_ERRS: &str = "win_errs";

const WINDOW_DURATION: u64 = 3600; // 1 hour rolling window
const ERROR_RATE_THRESHOLD_BPS: u32 = 5000; // 50% error rate trips alert


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

    // Window logic for metric decay/reset
    let start_key = Symbol::new(env, WINDOW_START);
    let win_ops_key = Symbol::new(env, WINDOW_OPS);
    let win_errs_key = Symbol::new(env, WINDOW_ERRS);
    
    let now = env.ledger().timestamp();
    let win_start_opt: Option<u64> = env.storage().persistent().get(&start_key);
    let win_start_val = win_start_opt.unwrap_or(now);
    
    // Explicit reset after WINDOW_DURATION or on first operation
    if win_start_opt.is_none() || now.saturating_sub(win_start_val) >= WINDOW_DURATION {
        env.storage().persistent().set(&start_key, &now);
        env.storage().persistent().set(&win_ops_key, &1u64);
        env.storage().persistent().set(&win_errs_key, &(if success { 0u64 } else { 1u64 }));
    } else {
        let w_ops: u64 = env.storage().persistent().get(&win_ops_key).unwrap_or(0);
        env.storage().persistent().set(&win_ops_key, &(w_ops + 1));
        
        if !success {
            let w_errs: u64 = env.storage().persistent().get(&win_errs_key).unwrap_or(0);
            env.storage().persistent().set(&win_errs_key, &(w_errs + 1));
        }
    }

    env.events().publish(
        (symbol_short!("metric"), symbol_short!("op")),
        OperationMetric {
            operation,
            caller,
            timestamp: now,
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

    let start_key = Symbol::new(env, WINDOW_START);
    let win_start_opt: Option<u64> = env.storage().persistent().get(&start_key);
    let win_start_val = win_start_opt.unwrap_or(0); // If none, then error rate check doesn't matter much, but we handle it
    let now = env.ledger().timestamp();
    
    // An alert triggered by a stale metric clears once the window decays
    let is_healthy = if win_start_opt.is_none() || now.saturating_sub(win_start_val) >= WINDOW_DURATION {
        true
    } else {
        let win_ops_key = Symbol::new(env, WINDOW_OPS);
        let win_errs_key = Symbol::new(env, WINDOW_ERRS);
        let w_ops: u64 = env.storage().persistent().get(&win_ops_key).unwrap_or(0);
        let w_errs: u64 = env.storage().persistent().get(&win_errs_key).unwrap_or(0);
        
        if w_ops > 0 {
            let error_rate = (w_errs as u128 * 10_000) / (w_ops as u128);
            error_rate < ERROR_RATE_THRESHOLD_BPS as u128
        } else {
            true
        }
    };


    HealthStatus {
        is_healthy,
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

const LARGE_PAYOUT_THRESHOLD: &str = "large_payout_threshold";
const DEFAULT_LARGE_PAYOUT_THRESHOLD_BPS: u32 = 1000;
const LARGE_PAYOUT_THRESHOLD_DENOMINATOR: i128 = 10_000;

pub fn set_large_payout_threshold_bps(env: &Env, threshold_bps: u32) {
    env.storage()
        .instance()
        .set(&Symbol::new(env, LARGE_PAYOUT_THRESHOLD), &threshold_bps);
}

pub fn get_large_payout_threshold_bps(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&Symbol::new(env, LARGE_PAYOUT_THRESHOLD))
        .unwrap_or(DEFAULT_LARGE_PAYOUT_THRESHOLD_BPS)
}

pub fn get_large_payout_threshold_amount(env: &Env, total_funds: i128) -> i128 {
    let threshold_bps = get_large_payout_threshold_bps(env) as i128;
    total_funds * threshold_bps / LARGE_PAYOUT_THRESHOLD_DENOMINATOR
}
