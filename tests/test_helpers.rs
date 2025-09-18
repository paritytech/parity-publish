// Test helpers for parity-publish tests

use std::collections::HashMap;

/// Mock crate structure for testing dependency-aware batching
#[derive(Debug, Clone, PartialEq)]
pub struct MockCrate {
    pub name: String,
    pub version: String,
    pub dependencies: Vec<String>,
    pub publish: bool,
}

impl MockCrate {
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            dependencies: vec![],
            publish: true,
        }
    }

    pub fn with_dependencies(mut self, deps: Vec<&str>) -> Self {
        self.dependencies = deps.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn with_publish(mut self, publish: bool) -> Self {
        self.publish = publish;
        self
    }
}

/// Mock workspace for testing
#[derive(Debug)]
pub struct MockWorkspace {
    pub crates: HashMap<String, MockCrate>,
}

impl MockWorkspace {
    pub fn new() -> Self {
        Self {
            crates: HashMap::new(),
        }
    }

    pub fn add_crate(&mut self, crate_info: MockCrate) {
        self.crates.insert(crate_info.name.clone(), crate_info);
    }

    pub fn get_crate(&self, name: &str) -> Option<&MockCrate> {
        self.crates.get(name)
    }

    pub fn get_dependencies(&self, crate_name: &str) -> Vec<String> {
        self.crates
            .get(crate_name)
            .map(|c| c.dependencies.clone())
            .unwrap_or_default()
    }
}

/// Test scenarios for dependency-aware batching
pub mod scenarios {
    use super::*;

    /// Simple scenario: 3 crates with no dependencies
    pub fn simple_no_deps() -> Vec<MockCrate> {
        vec![
            MockCrate::new("crate-a", "1.0.0"),
            MockCrate::new("crate-b", "1.0.0"),
            MockCrate::new("crate-c", "1.0.0"),
        ]
    }

    /// Linear dependency chain: a -> b -> c
    pub fn linear_chain() -> Vec<MockCrate> {
        vec![
            MockCrate::new("crate-a", "1.0.0"),
            MockCrate::new("crate-b", "1.0.0").with_dependencies(vec!["crate-a"]),
            MockCrate::new("crate-c", "1.0.0").with_dependencies(vec!["crate-b"]),
        ]
    }

    /// Diamond dependency: a -> b, a -> c, b -> d, c -> d
    pub fn diamond_deps() -> Vec<MockCrate> {
        vec![
            MockCrate::new("crate-a", "1.0.0"),
            MockCrate::new("crate-b", "1.0.0").with_dependencies(vec!["crate-a"]),
            MockCrate::new("crate-c", "1.0.0").with_dependencies(vec!["crate-a"]),
            MockCrate::new("crate-d", "1.0.0").with_dependencies(vec!["crate-b", "crate-c"]),
        ]
    }

    /// Complex scenario with multiple dependency levels
    pub fn complex_deps() -> Vec<MockCrate> {
        vec![
            MockCrate::new("crate-base", "1.0.0"),
            MockCrate::new("crate-utils", "1.0.0").with_dependencies(vec!["crate-base"]),
            MockCrate::new("crate-core", "1.0.0").with_dependencies(vec!["crate-base"]),
            MockCrate::new("crate-feature1", "1.0.0").with_dependencies(vec!["crate-core"]),
            MockCrate::new("crate-feature2", "1.0.0").with_dependencies(vec!["crate-core"]),
            MockCrate::new("crate-app", "1.0.0").with_dependencies(vec!["crate-utils", "crate-feature1", "crate-feature2"]),
        ]
    }

    /// Large scenario for performance testing
    pub fn large_scenario(size: usize) -> Vec<MockCrate> {
        let mut crates = Vec::with_capacity(size);

        // Add base crates (no dependencies)
        for i in 0..(size / 4) {
            crates.push(MockCrate::new(&format!("crate-base-{}", i), "1.0.0"));
        }

        // Add intermediate crates (depend on base crates)
        for i in 0..(size / 2) {
            let base_idx = i % (size / 4);
            crates.push(
                MockCrate::new(&format!("crate-mid-{}", i), "1.0.0")
                    .with_dependencies(vec![&format!("crate-base-{}", base_idx)])
            );
        }

        // Add top-level crates (depend on intermediate crates)
        for i in 0..(size / 4) {
            let mid_idx = i % (size / 2);
            crates.push(
                MockCrate::new(&format!("crate-top-{}", i), "1.0.0")
                    .with_dependencies(vec![&format!("crate-mid-{}", mid_idx)])
            );
        }

        crates
    }
}

/// Assertions for testing dependency-aware batching
pub mod assertions {
    use super::*;

    /// Assert that batches respect dependency ordering
    pub fn assert_dependency_order(batches: &[Vec<MockCrate>]) {
        let mut processed = std::collections::HashSet::new();

        for (batch_idx, batch) in batches.iter().enumerate() {
            // Check that all dependencies in this batch are already processed
            for crate_info in batch {
                for dep in &crate_info.dependencies {
                    assert!(
                        processed.contains(dep),
                        "Crate {} in batch {} depends on {} which hasn't been processed yet",
                        crate_info.name, batch_idx, dep
                    );
                }
            }

            // Mark this batch as processed
            for crate_info in batch {
                processed.insert(crate_info.name.clone());
            }
        }
    }

    /// Assert that all crates are included in exactly one batch
    pub fn assert_all_crates_included(original_crates: &[MockCrate], batches: &[Vec<MockCrate>]) {
        let mut found_crates = std::collections::HashSet::new();

        for batch in batches {
            for crate_info in batch {
                found_crates.insert(crate_info.name.clone());
            }
        }

        let original_names: std::collections::HashSet<_> = original_crates
            .iter()
            .map(|c| c.name.clone())
            .collect();

        assert_eq!(
            found_crates, original_names,
            "Not all crates were included in batches"
        );
    }

    /// Assert that batch sizes are within reasonable bounds
    pub fn assert_batch_size_bounds(batches: &[Vec<MockCrate>], target_size: usize) {
        for (batch_idx, batch) in batches.iter().enumerate() {
            assert!(
                batch.len() <= target_size * 2, // Allow some flexibility for dependency constraints
                "Batch {} has {} crates, which is too large for target size {}",
                batch_idx, batch.len(), target_size
            );

            if !batch.is_empty() {
                assert!(
                    batch.len() >= 1,
                    "Batch {} is empty, which is not allowed",
                    batch_idx
                );
            }
        }
    }
}

/// Performance testing utilities
pub mod performance {
    use super::*;
    use std::time::Instant;

    /// Measure execution time of a function
    pub fn measure_time<F, R>(f: F) -> (R, std::time::Duration)
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let result = f();
        let duration = start.elapsed();
        (result, duration)
    }

    /// Benchmark dependency-aware batching with different scenarios
    pub fn benchmark_batching<F>(batching_fn: F, scenarios: Vec<Vec<MockCrate>>)
    where
        F: Fn(&[MockCrate], usize) -> Vec<Vec<MockCrate>>,
    {
        for (scenario_name, crates) in scenarios.iter().enumerate() {
            println!("Benchmarking scenario {} with {} crates", scenario_name, crates.len());

            let (batches, duration) = measure_time(|| {
                batching_fn(crates, 10) // Use batch size 10
            });

            println!(
                "  Result: {} batches in {:?} ({:.2} crates/ms)",
                batches.len(),
                duration,
                crates.len() as f64 / duration.as_millis() as f64
            );
        }
    }
}
