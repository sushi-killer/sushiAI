//! `sushiai agents catalog [--json]`: what each agent CLI on this host offers —
//! its models, their reasoning levels, the levels the routine builder offers and
//! the permission modes. The table is `sushiai_agents::harness::catalog`; the
//! daemon's reader supplies this host's own Codex files. Needs no daemon.

use sushiai_daemon::agents_catalog;

pub fn run(args: impl Iterator<Item = String>) -> i32 {
    let args: Vec<String> = args.collect();
    if !args.is_empty() && args[0] != "catalog" {
        eprintln!("usage: sushiai agents catalog [--json]");
        return 1;
    }
    match print(&args) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("sushiai agents: {message}");
            1
        }
    }
}

fn print(args: &[String]) -> Result<(), String> {
    let catalogs = agents_catalog::agents();
    if args.iter().any(|a| a == "--json") {
        let text = serde_json::to_string_pretty(&catalogs).map_err(|e| e.to_string())?;
        println!("{text}");
        return Ok(());
    }
    for catalog in &catalogs {
        println!("{}\t{}", catalog.id, catalog.label);
        if catalog.models.is_empty() && catalog.efforts.is_empty() {
            continue;
        }
        println!(
            "\tmodels\t{}",
            catalog
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("\tefforts\t{}", catalog.efforts.join(", "));
        println!("\tbuilder\t{}", catalog.builder_efforts.join(", "));
        println!("\tmodes\t{}", catalog.permissions.join(", "));
        if !catalog.default_model.is_empty() {
            println!(
                "\tdefault\t{} ({})",
                catalog.default_model, catalog.default_effort
            );
        }
    }
    Ok(())
}
