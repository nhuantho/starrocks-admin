use super::models::{DiagnosticResult, ExecutionTree, RootCausePath};
use std::collections::{HashMap, HashSet, VecDeque};

/// Find a real downstream path from a diagnosed operator to an output. No synthetic edges.
pub(super) fn downstream_path(tree: &ExecutionTree, start: &str) -> Vec<String> {
    let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in &tree.nodes {
        for child in &node.children {
            parents.entry(child).or_default().push(&node.id);
        }
    }
    let mut queue = VecDeque::from([vec![start.to_string()]]);
    let mut seen = HashSet::from([start.to_string()]);
    while let Some(path) = queue.pop_front() {
        let last = path.last().unwrap();
        let next = parents.get(last.as_str());
        if last == &tree.root.id || next.is_none_or(|items| items.is_empty()) {
            return path;
        }
        for parent in next.unwrap() {
            if seen.insert(parent.to_string()) {
                let mut extended = path.clone();
                extended.push(parent.to_string());
                queue.push_back(extended);
            }
        }
    }
    vec![start.to_string()]
}

pub(super) fn build_paths(
    tree: Option<&ExecutionTree>,
    diagnostics: &[DiagnosticResult],
) -> Vec<RootCausePath> {
    let Some(tree) = tree else { return Vec::new() };
    let mut paths = Vec::new();
    let mut used = HashSet::new();
    for diagnostic in diagnostics {
        let Some(node) = tree.nodes.iter().find(|node| {
            node.plan_node_id == diagnostic.plan_node_id && node.plan_node_id.is_some()
        }) else {
            continue;
        };
        if !used.insert(node.id.clone()) {
            continue;
        }
        paths.push(RootCausePath {
            node_ids: downstream_path(tree, &node.id),
            message: diagnostic.message.clone(),
            evidence: vec![diagnostic.reason.clone()],
            suggestions: diagnostic.suggestions.clone(),
        });
        if paths.len() == 5 {
            break;
        }
    }
    paths
}
