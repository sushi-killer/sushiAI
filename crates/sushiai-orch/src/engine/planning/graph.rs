use crate::task::graph::has_cycle;
use crate::task::Task;
use std::collections::HashMap;

pub(in crate::engine) const MAX_SUBTASKS: usize = 8;

/// Whether the planner's split can become a graph: at least two parts,
/// unique non-empty keys, a request each, dependencies on known keys only,
/// and no cycle.
pub(in crate::engine) fn check_subtasks(
    subtasks: &[crate::engine::planning::draft::PlanSubtask],
) -> Result<(), String> {
    if subtasks.len() < 2 {
        return Err("a single part is the task itself".to_string());
    }
    if subtasks.len() > MAX_SUBTASKS {
        return Err(format!("more than {MAX_SUBTASKS} parts"));
    }
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for s in subtasks {
        let key = s.key.trim();
        if key.is_empty() || s.request.trim().is_empty() {
            return Err("a part without a key or a request".to_string());
        }
        if edges.contains_key(key) {
            return Err(format!("duplicate key {key}"));
        }
        edges.insert(key.to_string(), s.depends_on.clone());
    }
    for (key, deps) in &edges {
        if let Some(unknown) = deps.iter().find(|d| !edges.contains_key(d.trim())) {
            return Err(format!("{key} depends on unknown key {unknown}"));
        }
    }
    if has_cycle(&edges) {
        return Err("the parts depend on each other in a cycle".to_string());
    }
    Ok(())
}

pub(in crate::engine) fn path_components(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// The first pair of paths where one equals or is a directory prefix of the
/// other, compared per path component: returns the shorter (the shared area).
pub(in crate::engine) fn overlapping_path<'a>(a: &'a [String], b: &'a [String]) -> Option<&'a str> {
    for pa in a {
        let ca = path_components(pa);
        for pb in b {
            let cb = path_components(pb);
            let n = ca.len().min(cb.len());
            if ca[..n] == cb[..n] {
                return Some(if ca.len() <= cb.len() { pa } else { pb });
            }
        }
    }
    None
}

/// Whether `from` waits (directly or transitively) for `target`.
pub(in crate::engine) fn waits_for(
    deps: &HashMap<String, Vec<String>>,
    from: &str,
    target: &str,
) -> bool {
    let mut stack = vec![from.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(node) = stack.pop() {
        for d in deps.get(&node).into_iter().flatten() {
            if d == target {
                return true;
            }
            if seen.insert(d.clone()) {
                stack.push(d.clone());
            }
        }
    }
    false
}

/// Siblings that touch the same paths would conflict at landing, so the later
/// one in the list also waits for the earlier one unless they are already
/// ordered. A part without paths overlaps every sibling. Edges only go from
/// earlier to later positions, so no cycle can result. Returns the parts with
/// the added `dependsOn` keys and one decision line per added edge.
pub(in crate::engine) fn serialise_overlapping(
    parts: &[crate::engine::planning::draft::PlanSubtask],
) -> (
    Vec<crate::engine::planning::draft::PlanSubtask>,
    Vec<String>,
) {
    let mut out = parts.to_vec();
    let mut deps: HashMap<String, Vec<String>> = out
        .iter()
        .map(|p| {
            (
                p.key.trim().to_string(),
                p.depends_on.iter().map(|d| d.trim().to_string()).collect(),
            )
        })
        .collect();
    let mut decisions = Vec::new();
    for j in 1..out.len() {
        for i in 0..j {
            let (a, b) = (out[i].key.trim().to_string(), out[j].key.trim().to_string());
            if waits_for(&deps, &b, &a) || waits_for(&deps, &a, &b) {
                continue;
            }
            let shared: Option<String> = if out[i].paths.is_empty() || out[j].paths.is_empty() {
                Some("(no paths declared)".to_string())
            } else {
                overlapping_path(&out[i].paths, &out[j].paths).map(str::to_string)
            };
            let Some(shared) = shared else { continue };
            deps.get_mut(&b).unwrap().push(a.clone());
            out[j].depends_on.push(a.clone());
            decisions.push(format!("Serialised {b} after {a}: both touch {shared}"));
        }
    }
    (out, decisions)
}

/// Which earlier sibling each part continues as a relay, and one decision
/// line per link. Parts whose declared paths overlap and were serialised
/// run as one branch: a part continues the previous overlapping
/// part's commit instead of branching from the parent, so it edits on top of
/// that work and only the last part lands on the parent. A part only joins a
/// chain when every earlier part it overlaps and every part it depends on is
/// already in that chain and nothing continues the same predecessor;
/// otherwise it stays an ordinary serialised sibling. `parts` gain the
/// `dependsOn` a link needs, and a part that depends on a chain's middle
/// link also waits for the chain's last one, which is what lands.
pub(in crate::engine) fn relay_links(
    parts: &mut [crate::engine::planning::draft::PlanSubtask],
) -> (Vec<Option<usize>>, Vec<String>) {
    let keys: Vec<String> = parts.iter().map(|p| p.key.trim().to_string()).collect();
    // Only parts that both declare paths can be linked: a part without paths
    // is serialised, and its work lands on the parent like any sibling's.
    let overlaps = |i: usize, j: usize| {
        !parts[i].paths.is_empty()
            && !parts[j].paths.is_empty()
            && overlapping_path(&parts[i].paths, &parts[j].paths).is_some()
    };
    let mut link: Vec<Option<usize>> = vec![None; parts.len()];
    let mut has_next = vec![false; parts.len()];
    let chain_of = |link: &[Option<usize>], mut at: usize| {
        let mut chain = vec![at];
        while let Some(prev) = link[at] {
            chain.push(prev);
            at = prev;
        }
        chain
    };
    for j in 1..parts.len() {
        let earlier: Vec<usize> = (0..j).filter(|&i| overlaps(i, j)).collect();
        let Some(&p) = earlier.last() else { continue };
        if has_next[p] {
            continue;
        }
        let chain = chain_of(&link, p);
        let deps_in_chain = parts[j].depends_on.iter().all(|d| {
            keys.iter()
                .position(|k| k == d.trim())
                .is_some_and(|at| chain.contains(&at))
        });
        if earlier.iter().all(|i| chain.contains(i)) && deps_in_chain {
            link[j] = Some(p);
            has_next[p] = true;
        }
    }
    let mut decisions = Vec::new();
    for j in 0..parts.len() {
        let Some(p) = link[j] else { continue };
        if !parts[j].depends_on.iter().any(|d| d.trim() == keys[p]) {
            parts[j].depends_on.push(keys[p].clone());
        }
        decisions.push(format!(
            "Relay: {} continues on {}'s branch, one chain landed on the parent once",
            keys[j], keys[p]
        ));
    }
    // A part outside a chain that builds on one of its middle links needs
    // the chain's last link: that is the one that lands the work.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for i in 0..parts.len() {
        if link[i].is_some() {
            continue;
        }
        let mut extra = Vec::new();
        for d in parts[i].depends_on.clone() {
            let Some(mut at) = keys.iter().position(|k| k == d.trim()) else {
                continue;
            };
            let mut moved = false;
            while let Some(next) = (0..parts.len()).find(|&n| link[n] == Some(at)) {
                at = next;
                moved = true;
            }
            if moved {
                extra.push(keys[at].clone());
            }
        }
        for key in extra {
            if !parts[i].depends_on.iter().any(|d| d.trim() == key) {
                parts[i].depends_on.push(key);
            }
        }
    }
    for part in parts.iter() {
        edges.insert(
            part.key.trim().to_string(),
            part.depends_on
                .iter()
                .map(|d| d.trim().to_string())
                .collect(),
        );
    }
    if has_cycle(&edges) {
        return (vec![None; parts.len()], Vec::new());
    }
    (link, decisions)
}

/// A subtask's request: the planner's text first (what its own planner
/// drafts from), then the whole it belongs to and what was already decided
/// for it.
pub(in crate::engine) fn subtask_request(parent: &Task, part: &str) -> String {
    let mut out = format!(
        "{}\n\nThis is one part of a larger task, \"{}\": {}",
        part.trim(),
        parent.title,
        parent.goal
    );
    let decided: Vec<&String> = parent
        .decisions
        .iter()
        .filter(|d| {
            d.starts_with("Owner:") || d.starts_with("Lead:") || d.starts_with("Orchestrator:")
        })
        .collect();
    if !decided.is_empty() {
        out.push_str("\n\nAlready decided for the whole task:\n");
        for d in decided {
            out.push_str(&format!("- {d}\n"));
        }
    }
    out
}
