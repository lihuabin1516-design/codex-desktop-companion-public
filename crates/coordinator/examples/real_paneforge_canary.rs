use std::env;

use coordination_coordinator::{parse_real_paneforge_canary_args, run_real_paneforge_canary};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_real_paneforge_canary_args(env::args_os().skip(1))?;
    let report = run_real_paneforge_canary(args)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
