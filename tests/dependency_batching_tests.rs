// Comprehensive tests for dependency-aware batching functionality

mod test_helpers;
use test_helpers::{MockCrate, MockWorkspace, scenarios, assertions, performance};

/// Test the core dependency-aware batching logic
#[test]
fn test_dependency_aware_batching_core() {
    // Test with simple linear dependency chain
    let crates = scenarios::linear_chain();
    let batches = simulate_dependency_aware_batching(&crates, 2);

    // Should create 3 batches due to dependencies
    assert_eq!(batches.len(), 3);

    // First batch: crate-a (no dependencies)
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[0][0].name, "crate-a");

    // Second batch: crate-b (depends on a)
    assert_eq!(batches[1].len(), 1);
    assert_eq!(batches[1][0].name, "crate-b");

    // Third batch: crate-c (depends on b)
    assert_eq!(batches[2].len(), 1);
    assert_eq!(batches[2][0].name, "crate-c");

    // Verify dependency ordering
    assertions::assert_dependency_order(&batches);
    assertions::assert_all_crates_included(&crates, &batches);
}

#[test]
fn test_dependency_aware_batching_diamond() {
    // Test with diamond dependency pattern
    let crates = scenarios::diamond_deps();
    let batches = simulate_dependency_aware_batching(&crates, 2);

    // Should create at least 3 batches due to dependencies
    assert!(batches.len() >= 3);

    // First batch: crate-a (no dependencies)
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[0][0].name, "crate-a");

    // Second batch: crate-b and crate-c (both depend on a)
    assert_eq!(batches[1].len(), 2);
    assert!(batches[1].iter().any(|c| c.name == "crate-b"));
    assert!(batches[1].iter().any(|c| c.name == "crate-c"));

    // Third batch: crate-d (depends on b and c)
    assert_eq!(batches[2].len(), 1);
    assert_eq!(batches[2][0].name, "crate-d");

    // Verify dependency ordering
    assertions::assert_dependency_order(&batches);
    assertions::assert_all_crates_included(&crates, &batches);
}

#[test]
fn test_dependency_aware_batching_complex() {
    // Test with complex dependency scenario
    let crates = scenarios::complex_deps();
    let batches = simulate_dependency_aware_batching(&crates, 3);

    // Should create multiple batches due to dependencies
    assert!(batches.len() >= 3);

    // First batch: crate-base (no dependencies)
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[0][0].name, "crate-base");

    // Verify dependency ordering
    assertions::assert_dependency_order(&batches);
    assertions::assert_all_crates_included(&crates, &batches);
    assertions::assert_batch_size_bounds(&batches, 3);
}

#[test]
fn test_dependency_aware_batching_no_deps() {
    // Test with crates that have no dependencies
    let crates = scenarios::simple_no_deps();
    let batches = simulate_dependency_aware_batching(&crates, 2);

    // Should create 2 batches with 2 and 1 crates respectively
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].len(), 2);
    assert_eq!(batches[1].len(), 1);

    // Verify all crates are included
    assertions::assert_all_crates_included(&crates, &batches);
}

#[test]
fn test_dependency_aware_batching_empty_input() {
    // Test with empty input
    let crates: Vec<MockCrate> = vec![];
    let batches = simulate_dependency_aware_batching(&crates, 5);

    assert_eq!(batches.len(), 0);
}

#[test]
fn test_dependency_aware_batching_single_crate() {
    // Test with single crate
    let crates = vec![MockCrate::new("single-crate", "1.0.0")];
    let batches = simulate_dependency_aware_batching(&crates, 5);

    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[0][0].name, "single-crate");
}

#[test]
fn test_dependency_aware_batching_large_batch_size() {
    // Test with batch size larger than total crates
    let crates = scenarios::simple_no_deps();
    let batches = simulate_dependency_aware_batching(&crates, 10);

    // Should create 1 batch since batch size is larger than total crates
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 3);
}

#[test]
fn test_dependency_aware_batching_performance() {
    // Test performance with different scenarios
    let scenarios = vec![
        ("small", scenarios::simple_no_deps()),
        ("linear", scenarios::linear_chain()),
        ("diamond", scenarios::diamond_deps()),
        ("complex", scenarios::complex_deps()),
        ("large", scenarios::large_scenario(100)),
    ];

    // Benchmark the batching function
    performance::benchmark_batching(simulate_dependency_aware_batching, scenarios.into_iter().map(|(_, crates)| crates).collect());
}

#[test]
fn test_dependency_aware_batching_edge_cases() {
    // Test various edge cases

    // Circular dependencies (should be handled gracefully)
    let crates = vec![
        MockCrate::new("crate-a", "1.0.0").with_dependencies(vec!["crate-b"]),
        MockCrate::new("crate-b", "1.0.0").with_dependencies(vec!["crate-a"]),
    ];

    let batches = simulate_dependency_aware_batching(&crates, 2);

    // Should still create batches (circular deps will be in same batch)
    assert!(batches.len() > 0);

    // Self-dependencies
    let crates = vec![
        MockCrate::new("crate-self", "1.0.0").with_dependencies(vec!["crate-self"]),
    ];

    let batches = simulate_dependency_aware_batching(&crates, 1);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 1);
}

#[test]
fn test_dependency_aware_batching_mixed_publish_flags() {
    // Test with mixed publish flags
    let crates = vec![
        MockCrate::new("crate-a", "1.0.0").with_publish(true),
        MockCrate::new("crate-b", "1.0.0").with_publish(false), // Should be filtered out
        MockCrate::new("crate-c", "1.0.0").with_publish(true),
    ];

    // Filter to only publishable crates
    let publishable_crates: Vec<&MockCrate> = crates.iter().filter(|c| c.publish).collect();
    assert_eq!(publishable_crates.len(), 2);

    // Convert to owned MockCrate for the simulation function
    let owned_crates: Vec<MockCrate> = publishable_crates.iter().map(|c| (*c).clone()).collect();
    let batches = simulate_dependency_aware_batching(&owned_crates, 2);
    assert_eq!(batches.len(), 1); // Both crates have no deps, so they can be in same batch
    assert_eq!(batches[0].len(), 2);
}

/// Simulate the dependency-aware batching algorithm for testing
fn simulate_dependency_aware_batching(crates: &[MockCrate], target_batch_size: usize) -> Vec<Vec<MockCrate>> {
    let mut batches = Vec::new();
    let mut remaining_crates: Vec<&MockCrate> = crates.iter().collect();
    let mut processed = std::collections::HashSet::new();

    while !remaining_crates.is_empty() {
        let mut current_batch = Vec::new();
        let mut i = 0;

        while i < remaining_crates.len() && current_batch.len() < target_batch_size {
            let crate_info = &remaining_crates[i];

            // Check if all dependencies are processed
            let deps_ready = crate_info.dependencies.iter().all(|dep| processed.contains(dep));

            if deps_ready {
                current_batch.push((*crate_info).clone());
                remaining_crates.remove(i);
            } else {
                i += 1;
            }
        }

        if current_batch.is_empty() {
            // If we can't add any crates to this batch, we have a circular dependency
            // or all remaining crates depend on each other. Add them all to the current batch.
            let mut forced_batch = Vec::new();
            for crate_info in remaining_crates.drain(..) {
                forced_batch.push((*crate_info).clone());
                processed.insert(crate_info.name.clone());
            }
            if !forced_batch.is_empty() {
                batches.push(forced_batch);
            }
        } else {
            // Mark the current batch as processed before moving it
            for crate_info in &current_batch {
                processed.insert(crate_info.name.clone());
            }
            batches.push(current_batch);
        }
    }

    batches
}

/// Test the mock workspace functionality
#[test]
fn test_mock_workspace() {
    let mut workspace = MockWorkspace::new();

    let crate_a = MockCrate::new("crate-a", "1.0.0");
    let crate_b = MockCrate::new("crate-b", "1.0.0").with_dependencies(vec!["crate-a"]);

    workspace.add_crate(crate_a.clone());
    workspace.add_crate(crate_b.clone());

    assert_eq!(workspace.get_crate("crate-a"), Some(&crate_a));
    assert_eq!(workspace.get_crate("crate-b"), Some(&crate_b));
    assert_eq!(workspace.get_crate("crate-c"), None);

    assert_eq!(workspace.get_dependencies("crate-a"), Vec::<String>::new());
    assert_eq!(workspace.get_dependencies("crate-b"), vec!["crate-a".to_string()]);
}

/// Test the mock crate builder pattern
#[test]
fn test_mock_crate_builder() {
    let crate_info = MockCrate::new("test-crate", "1.0.0")
        .with_dependencies(vec!["dep1", "dep2"])
        .with_publish(false);

    assert_eq!(crate_info.name, "test-crate");
    assert_eq!(crate_info.version, "1.0.0");
    assert_eq!(crate_info.dependencies, vec!["dep1".to_string(), "dep2".to_string()]);
    assert_eq!(crate_info.publish, false);
}
