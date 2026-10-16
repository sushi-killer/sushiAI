use crate::clip::truncate_chars;
use crate::engine::planning::graph::overlapping_path;
use crate::task::graph::has_cycle;
use crate::task::ChecklistItem;
use std::collections::HashMap;

const MAX_ITEM_CHARS: usize = 300;

/// What the plan stage does with the planner's subtasks: parts that keep
/// their own child, and the ordered parts of the one merged task.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::engine) struct MergePlan {
    pub separate: Vec<usize>,
    pub merged: Vec<usize>,
}

/// A part stays its own child when it lists paths and targeted proof.
/// Overlapping complete drafts are serialised; incomplete overlaps become
/// one merged group, in planner
/// order made stable-topological by `dependsOn`. If the group, as one node,
/// would make the dependency graph cyclic, everything merges.
pub(in crate::engine) fn merge_parts(
    parts: &[crate::engine::planning::draft::PlanSubtask],
) -> MergePlan {
    let keys: Vec<&str> = parts.iter().map(|p| p.key.trim()).collect();
    let stands_alone = |i: usize| {
        !parts[i].paths.is_empty()
            && !parts[i].check_command().is_empty()
            && !(0..parts.len()).any(|j| {
                j != i
                    && overlapping_path(&parts[i].paths, &parts[j].paths).is_some()
                    && !(parts[i].is_complete() && parts[j].is_complete())
            })
    };
    let mut separate: Vec<usize> = (0..parts.len()).filter(|&i| stands_alone(i)).collect();
    let mut merged: Vec<usize> = (0..parts.len()).filter(|i| !separate.contains(i)).collect();
    if !merged.is_empty() && !separate.is_empty() {
        const GROUP: &str = "\u{0}group";
        let node = |key: &str, group: &[usize]| -> String {
            match keys.iter().position(|k| *k == key) {
                Some(at) if group.contains(&at) => GROUP.to_string(),
                _ => key.to_string(),
            }
        };
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        for (i, part) in parts.iter().enumerate() {
            let from = if merged.contains(&i) {
                GROUP.to_string()
            } else {
                keys[i].to_string()
            };
            let entry = edges.entry(from.clone()).or_default();
            for d in &part.depends_on {
                let to = node(d.trim(), &merged);
                if to != from {
                    entry.push(to);
                }
            }
        }
        if has_cycle(&edges) {
            merged = (0..parts.len()).collect();
            separate.clear();
        }
    }
    // Stable topological order inside the group.
    let mut ordered: Vec<usize> = Vec::new();
    let mut left = merged.clone();
    while !left.is_empty() {
        let next = left
            .iter()
            .position(|&i| {
                parts[i].depends_on.iter().all(|d| {
                    keys.iter()
                        .position(|k| *k == d.trim())
                        .is_none_or(|at| !left.contains(&at) || at == i)
                })
            })
            .unwrap_or(0);
        ordered.push(left.remove(next));
    }
    MergePlan {
        separate,
        merged: ordered,
    }
}

/// The checklist of a merged group, in group order.
pub(in crate::engine) fn checklist_of(
    parts: &[crate::engine::planning::draft::PlanSubtask],
    merged: &[usize],
) -> Vec<ChecklistItem> {
    merged
        .iter()
        .map(|&i| ChecklistItem {
            text: truncate_chars(parts[i].request.trim(), MAX_ITEM_CHARS),
            check: parts[i].check_command().to_string(),
            passes: false,
        })
        .collect()
}

#[cfg(test)]
mod tests;
