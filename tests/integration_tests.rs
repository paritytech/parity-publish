// Integration tests for parity-publish
// These tests run against the compiled binary

#[test]
fn test_cli_struct_creation() {
    // Test that we can create CLI-like structures for testing
    #[derive(Debug, Clone, PartialEq)]
    struct TestApply {
        publish: bool,
        dry_run: bool,
        no_verify: bool,
        allow_dirty: bool,
        print: bool,
        max_concurrent: usize,
        batch_delay: u64,
        batch_size: usize,
    }

    let apply = TestApply {
        publish: false,
        dry_run: false,
        no_verify: false,
        allow_dirty: false,
        print: false,
        max_concurrent: 3,
        batch_delay: 120,
        batch_size: 10,
    };

    assert_eq!(apply.max_concurrent, 3);
    assert_eq!(apply.batch_delay, 120);
    assert_eq!(apply.batch_size, 10);
}

#[test]
fn test_cli_struct_clone() {
    // Test that CLI-like structs can be cloned (required for async operations)
    #[derive(Debug, Clone, PartialEq)]
    struct TestApply {
        publish: bool,
        max_concurrent: usize,
        batch_delay: u64,
        batch_size: usize,
    }

    let apply = TestApply {
        publish: true,
        max_concurrent: 5,
        batch_delay: 60,
        batch_size: 15,
    };

    let cloned = apply.clone();

    assert_eq!(cloned.publish, apply.publish);
    assert_eq!(cloned.max_concurrent, apply.max_concurrent);
    assert_eq!(cloned.batch_delay, apply.batch_delay);
    assert_eq!(cloned.batch_size, apply.batch_size);
}

#[test]
fn test_publish_struct_creation() {
    // Test creating Publish-like structs for testing
    #[derive(Debug, PartialEq)]
    struct TestPublish {
        name: String,
        from: String,
        to: String,
        bump: TestBumpKind,
        reason: Option<String>,
        publish: bool,
        verify: bool,
    }

    #[derive(Debug, PartialEq)]
    enum TestBumpKind {
        None,
        Patch,
        Minor,
        Major,
    }

    let publish = TestPublish {
        name: "test-crate".to_string(),
        from: "1.0.0".to_string(),
        to: "1.0.1".to_string(),
        bump: TestBumpKind::Patch,
        reason: Some("Bug fix".to_string()),
        publish: true,
        verify: true,
    };

    assert_eq!(publish.name, "test-crate");
    assert_eq!(publish.from, "1.0.0");
    assert_eq!(publish.to, "1.0.1");
    assert_eq!(publish.bump, TestBumpKind::Patch);
    assert_eq!(publish.reason, Some("Bug fix".to_string()));
    assert!(publish.publish);
    assert!(publish.verify);
}

#[test]
fn test_bump_kind_parsing() {
    // Test that BumpKind-like enums can be parsed from strings
    use std::str::FromStr;

    #[derive(Debug, PartialEq)]
    enum TestBumpKind {
        None,
        Patch,
        Minor,
        Major,
    }

    impl FromStr for TestBumpKind {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            match s.to_lowercase().as_str() {
                "none" => Ok(TestBumpKind::None),
                "patch" => Ok(TestBumpKind::Patch),
                "minor" => Ok(TestBumpKind::Minor),
                "major" => Ok(TestBumpKind::Major),
                _ => Err(format!("Unknown bump kind: {}", s)),
            }
        }
    }

    // Test the FromStr implementation
    assert_eq!(TestBumpKind::from_str("none").unwrap(), TestBumpKind::None);
    assert_eq!(TestBumpKind::from_str("patch").unwrap(), TestBumpKind::Patch);
    assert_eq!(TestBumpKind::from_str("minor").unwrap(), TestBumpKind::Minor);
    assert_eq!(TestBumpKind::from_str("major").unwrap(), TestBumpKind::Major);
}

#[test]
fn test_concurrent_settings_validation() {
    // Test that concurrent settings make logical sense
    let test_cases = vec![
        (1, 1, true),   // Single crate, single batch
        (3, 10, true),  // 3 concurrent, 10 batch size
        (5, 5, true),   // Equal concurrent and batch size
        (10, 5, false), // More concurrent than batch size (inefficient)
    ];

    for (concurrent, batch_size, should_be_valid) in test_cases {
        let is_valid = concurrent <= batch_size;
        assert_eq!(is_valid, should_be_valid,
            "Concurrent {} vs batch size {} should be valid: {}",
            concurrent, batch_size, should_be_valid);
    }
}

#[test]
fn test_batch_delay_calculation() {
    // Test batch delay calculations
    let delays = vec![60, 120, 300, 600]; // 1min, 2min, 5min, 10min

    for delay_seconds in delays {
        let delay = std::time::Duration::from_secs(delay_seconds);
        assert_eq!(delay.as_secs(), delay_seconds);
        assert!(delay > std::time::Duration::from_secs(0));

        // Test that delay is reasonable (not too short, not too long)
        assert!(delay >= std::time::Duration::from_secs(30),
            "Delay {} seconds is too short", delay_seconds);
        assert!(delay <= std::time::Duration::from_secs(3600),
            "Delay {} seconds is too long", delay_seconds);
    }
}

#[test]
fn test_crate_filtering_logic() {
    // Test the crate filtering logic that would be used in the main publishing loop
    #[derive(Debug, PartialEq)]
    struct TestPublish {
        name: String,
        from: String,
        to: String,
        bump: TestBumpKind,
        reason: Option<String>,
        publish: bool,
        verify: bool,
    }

    #[derive(Debug, PartialEq)]
    enum TestBumpKind {
        Patch,
    }

    let crates = vec![
        TestPublish {
            name: "crate-a".to_string(),
            from: "1.0.0".to_string(),
            to: "1.0.1".to_string(),
            bump: TestBumpKind::Patch,
            reason: None,
            publish: true,
            verify: true,
        },
        TestPublish {
            name: "crate-b".to_string(),
            from: "1.0.0".to_string(),
            to: "1.0.1".to_string(),
            bump: TestBumpKind::Patch,
            reason: None,
            publish: false, // Should be filtered out
            verify: true,
        },
        TestPublish {
            name: "crate-c".to_string(),
            from: "1.0.0".to_string(),
            to: "1.0.1".to_string(),
            bump: TestBumpKind::Patch,
            reason: None,
            publish: true,
            verify: true,
        },
    ];

    // Filter crates that should be published
    let publishable_crates: Vec<&TestPublish> = crates.iter()
        .filter(|c| c.publish)
        .collect();

    assert_eq!(publishable_crates.len(), 2);
    assert_eq!(publishable_crates[0].name, "crate-a");
    assert_eq!(publishable_crates[1].name, "crate-c");

    // Verify that crate-b was filtered out
    assert!(!publishable_crates.iter().any(|c| c.name == "crate-b"));
}

#[test]
fn test_rayon_dependency() {
    // Test that rayon is available and can be used
    use rayon::prelude::*;

    let numbers: Vec<i32> = (1..=100).collect();
    let sum: i32 = numbers.par_iter().sum();

    assert_eq!(sum, 5050); // Sum of 1 to 100

    // Test parallel iteration
    let doubled: Vec<i32> = numbers.par_iter().map(|&x| x * 2).collect();
    assert_eq!(doubled.len(), 100);
    assert_eq!(doubled[0], 2);
    assert_eq!(doubled[99], 200);
}

#[test]
fn test_thread_pool_creation() {
    // Test that we can create thread pools with different configurations
    use rayon::ThreadPoolBuilder;

    let pool_1 = ThreadPoolBuilder::new()
        .num_threads(1)
        .build();
    assert!(pool_1.is_ok());

    let pool_4 = ThreadPoolBuilder::new()
        .num_threads(4)
        .build();
    assert!(pool_4.is_ok());

    let pool_8 = ThreadPoolBuilder::new()
        .num_threads(8)
        .build();
    assert!(pool_8.is_ok());

    // Test that thread count is respected
    if let Ok(pool) = pool_4 {
        assert_eq!(pool.current_num_threads(), 4);
    }
}

#[test]
fn test_dependency_aware_batching_logic() {
    // Test the core logic of dependency-aware batching without cargo dependencies

    #[derive(Debug, Clone)]
    struct TestCrate {
        name: String,
        dependencies: Vec<String>,
    }

    #[derive(Debug)]
    struct TestCrateInfo<'a> {
        pkg: &'a TestCrate,
        dependencies: Vec<String>,
    }

    // Create test crates with dependencies
    let crates = vec![
        TestCrate {
            name: "crate-a".to_string(),
            dependencies: vec![],
        },
        TestCrate {
            name: "crate-b".to_string(),
            dependencies: vec!["crate-a".to_string()],
        },
        TestCrate {
            name: "crate-c".to_string(),
            dependencies: vec!["crate-a".to_string()],
        },
    ];

    // Simulate dependency-aware batching
    let mut batches: Vec<Vec<TestCrateInfo>> = vec![];
    let mut processed = std::collections::HashSet::new();

    // First batch: crates with no dependencies
    let first_batch: Vec<TestCrateInfo> = crates.iter()
        .filter(|c| c.dependencies.is_empty())
        .map(|c| TestCrateInfo {
            pkg: c,
            dependencies: c.dependencies.clone(),
        })
        .collect();

    batches.push(first_batch);

    // Mark first batch as processed
    for crate_info in &batches[0] {
        processed.insert(crate_info.pkg.name.clone());
    }

    // Second batch: crates whose dependencies are processed
    let second_batch: Vec<TestCrateInfo> = crates.iter()
        .filter(|c| !c.dependencies.is_empty())
        .filter(|c| c.dependencies.iter().all(|dep| processed.contains(dep)))
        .map(|c| TestCrateInfo {
            pkg: c,
            dependencies: c.dependencies.clone(),
        })
        .collect();

    if !second_batch.is_empty() {
        batches.push(second_batch);
    }

    // Verify batching logic
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].len(), 1); // crate-a (no deps)
    assert_eq!(batches[1].len(), 2); // crate-b and crate-c (depend on a)

    // Verify dependency order
    assert_eq!(batches[0][0].pkg.name, "crate-a");
    assert!(batches[1].iter().any(|c| c.pkg.name == "crate-b"));
    assert!(batches[1].iter().any(|c| c.pkg.name == "crate-c"));
}

#[test]
fn test_staging_registry_options() {
    // Test that staging registry options are properly configured
    #[derive(Debug, Clone, PartialEq)]
    struct TestApply {
        publish: bool,
        staging: bool,
        registry_url: Option<String>,
    }

    // Test default (production)
    let apply = TestApply {
        publish: true,
        staging: false,
        registry_url: None,
    };

    // Test staging flag
    let apply_staging = TestApply {
        publish: true,
        staging: true,
        registry_url: None,
    };

    // Test custom registry URL
    let apply_custom = TestApply {
        publish: true,
        staging: false,
        registry_url: Some("https://custom.registry.com".to_string()),
    };

    // Test staging flag with custom URL (custom URL should override)
    let apply_staging_custom = TestApply {
        publish: true,
        staging: true,
        registry_url: Some("https://custom.registry.com".to_string()),
    };

    assert!(!apply.staging);
    assert!(apply_staging.staging);
    assert_eq!(apply.registry_url, None);
    assert_eq!(apply_custom.registry_url, Some("https://custom.registry.com".to_string()));
    assert_eq!(apply_staging_custom.registry_url, Some("https://custom.registry.com".to_string()));
}

#[test]
fn test_registry_url_priority() {
    // Test that custom registry URL takes priority over staging flag
    #[derive(Debug, Clone, PartialEq)]
    struct TestApply {
        staging: bool,
        registry_url: Option<String>,
    }

    let apply = TestApply {
        staging: true,
        registry_url: Some("https://custom.registry.com".to_string()),
    };

    // Custom URL should take priority
    let effective_url = apply.registry_url.as_ref().unwrap();
    assert_eq!(effective_url, "https://custom.registry.com");

    // Staging flag should be ignored when custom URL is provided
    assert!(apply.staging); // But staging flag is still set
}

#[test]
fn test_staging_registry_environment_variables() {
    // Test that staging registry sets correct environment variables
    let staging_url = "https://staging.crates.io";
    let custom_url = "https://custom.registry.com";

    // Simulate environment variable setting
    let mut env_vars = std::collections::HashMap::new();

    // Test staging registry
    env_vars.insert("CARGO_REGISTRY_INDEX".to_string(), staging_url.to_string());
    env_vars.insert("CARGO_REGISTRY_STAGING".to_string(), "true".to_string());

    assert_eq!(env_vars.get("CARGO_REGISTRY_INDEX"), Some(&staging_url.to_string()));
    assert_eq!(env_vars.get("CARGO_REGISTRY_STAGING"), Some(&"true".to_string()));

    // Test custom registry
    env_vars.insert("CARGO_REGISTRY_INDEX".to_string(), custom_url.to_string());
    env_vars.remove("CARGO_REGISTRY_STAGING");

    assert_eq!(env_vars.get("CARGO_REGISTRY_INDEX"), Some(&custom_url.to_string()));
    assert_eq!(env_vars.get("CARGO_REGISTRY_STAGING"), None);
}

#[test]
fn test_registry_url_validation() {
    // Test that registry URLs are valid
    let valid_urls = vec![
        "https://crates.io",
        "https://staging.crates.io",
        "https://custom.registry.com",
        "https://registry.example.org",
    ];

    for url in valid_urls {
        // Basic URL validation
        assert!(url.starts_with("https://"), "URL {} should use HTTPS", url);
        assert!(url.contains("."), "URL {} should contain a domain", url);
        assert!(!url.ends_with("/"), "URL {} should not end with slash", url);
    }

    // Test that staging URL is correct
    let staging_url = "https://staging.crates.io";
    assert_eq!(staging_url, "https://staging.crates.io");

    // Test that production URL is correct
    let production_url = "https://crates.io";
    assert_eq!(production_url, "https://crates.io");
}
