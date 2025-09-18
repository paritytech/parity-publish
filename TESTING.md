# Testing Documentation for Parity-Publish

This document describes the comprehensive test suite for the dependency-aware parallel publishing functionality.

## Test Structure

The test suite consists of three main components:

1. **Unit Tests** (`src/apply.rs`): Tests for the core functionality
2. **Integration Tests** (`tests/integration_tests.rs`): Tests for CLI and general functionality
3. **Dependency Batching Tests** (`tests/dependency_batching_tests.rs`): Comprehensive tests for dependency-aware batching
4. **Test Helpers** (`tests/test_helpers.rs`): Utilities and mock objects for testing

## Running Tests

### All Tests
```bash
cargo test
```

### Specific Test Suites
```bash
# Unit tests only
cargo test --lib

# Integration tests only
cargo test --test integration_tests

# Dependency batching tests only
cargo test --test dependency_batching_tests
```

### Individual Tests
```bash
# Run a specific test
cargo test test_dependency_aware_batching_core

# Run with output (for debugging)
cargo test test_dependency_aware_batching_core -- --nocapture
```

## Test Coverage

### 1. Unit Tests (`src/apply.rs`)

Tests the core functionality of the `apply` module:

- **`test_get_crate_dependencies`**: Tests dependency extraction from workspace
- **`test_create_dependency_aware_batches_simple`**: Tests basic batching without dependencies
- **`test_create_dependency_aware_batches_with_dependencies`**: Tests batching with dependencies
- **`test_create_dependency_aware_batches_empty_input`**: Tests edge case with empty input
- **`test_create_dependency_aware_batches_single_crate`**: Tests single crate scenario
- **`test_create_dependency_aware_batches_large_batch_size`**: Tests large batch size handling
- **`test_crate_info_structure`**: Tests the `CrateInfo` struct
- **`test_dependency_ordering`**: Tests dependency ordering logic

### 2. Integration Tests (`tests/integration_tests.rs`)

Tests the CLI interface and general functionality:

- **`test_cli_struct_creation`**: Tests CLI struct creation
- **`test_cli_struct_clone`**: Tests CLI struct cloning (required for async operations)
- **`test_publish_struct_creation`**: Tests Publish struct creation
- **`test_bump_kind_parsing`**: Tests version bump parsing
- **`test_concurrent_settings_validation`**: Tests concurrent settings validation
- **`test_batch_delay_calculation`**: Tests batch delay calculations
- **`test_crate_filtering_logic`**: Tests crate filtering logic
- **`test_rayon_dependency`**: Tests rayon parallelization library
- **`test_thread_pool_creation`**: Tests thread pool creation
- **`test_dependency_aware_batching_logic`**: Tests core batching logic

### 3. Dependency Batching Tests (`tests/dependency_batching_tests.rs`)

Comprehensive tests for the dependency-aware batching algorithm:

#### Core Functionality Tests
- **`test_dependency_aware_batching_core`**: Tests linear dependency chain (a → b → c)
- **`test_dependency_aware_batching_diamond`**: Tests diamond dependency pattern
- **`test_dependency_aware_batching_complex`**: Tests complex multi-level dependencies
- **`test_dependency_aware_batching_no_deps`**: Tests crates with no dependencies

#### Edge Case Tests
- **`test_dependency_aware_batching_empty_input`**: Tests empty input handling
- **`test_dependency_aware_batching_single_crate`**: Tests single crate scenario
- **`test_dependency_aware_batching_large_batch_size`**: Tests large batch size handling
- **`test_dependency_aware_batching_edge_cases`**: Tests circular dependencies and self-dependencies
- **`test_dependency_aware_batching_mixed_publish_flags`**: Tests mixed publish flags

#### Utility Tests
- **`test_mock_workspace`**: Tests mock workspace functionality
- **`test_mock_crate_builder`**: Tests mock crate builder pattern
- **`test_dependency_aware_batching_performance`**: Tests performance with various scenarios

### 4. Test Helpers (`tests/test_helpers.rs`)

Provides utilities for testing:

#### Mock Objects
- **`MockCrate`**: Mock crate structure with builder pattern
- **`MockWorkspace`**: Mock workspace for dependency testing

#### Test Scenarios
- **`scenarios::simple_no_deps`**: 3 crates with no dependencies
- **`scenarios::linear_chain`**: Linear dependency chain (a → b → c)
- **`scenarios::diamond_deps`**: Diamond dependency pattern
- **`scenarios::complex_deps`**: Complex multi-level dependencies
- **`scenarios::large_scenario`**: Large scenario for performance testing

#### Assertions
- **`assertions::assert_dependency_order`**: Verifies dependency ordering
- **`assertions::assert_all_crates_included`**: Verifies all crates are included
- **`assertions::assert_batch_size_bounds`**: Verifies batch size constraints

#### Performance Testing
- **`performance::measure_time`**: Measures execution time
- **`performance::benchmark_batching`**: Benchmarks batching performance

## Test Scenarios

### 1. Simple No Dependencies
```
crate-a (no deps)
crate-b (no deps)  
crate-c (no deps)
```
**Expected**: 2 batches with 2 and 1 crates respectively

### 2. Linear Dependency Chain
```
crate-a (no deps)
crate-b (depends on a)
crate-c (depends on b)
```
**Expected**: 3 batches, one crate per batch

### 3. Diamond Dependencies
```
crate-a (no deps)
crate-b (depends on a)
crate-c (depends on a)
crate-d (depends on b and c)
```
**Expected**: 3 batches
- Batch 0: crate-a
- Batch 1: crate-b, crate-c
- Batch 2: crate-d

### 4. Complex Multi-Level
```
crate-base (no deps)
crate-utils (depends on base)
crate-core (depends on base)
crate-feature1 (depends on core)
crate-feature2 (depends on core)
crate-app (depends on utils, feature1, feature2)
```
**Expected**: Multiple batches respecting dependency levels

## Test Assertions

### Dependency Order Verification
```rust
assertions::assert_dependency_order(&batches);
```
Ensures that all dependencies of a crate in a given batch have been processed in previous batches.

### Completeness Verification
```rust
assertions::assert_all_crates_included(&crates, &batches);
```
Ensures that all crates from the input are included in exactly one batch.

### Batch Size Verification
```rust
assertions::assert_batch_size_bounds(&batches, target_size);
```
Ensures that batch sizes are within reasonable bounds (allowing flexibility for dependency constraints).

## Performance Testing

The test suite includes performance benchmarks:

```rust
performance::benchmark_batching(simulate_dependency_aware_batching, scenarios);
```

This tests the batching algorithm with different scenarios and reports:
- Number of batches created
- Execution time
- Crates processed per millisecond

## Edge Cases Covered

1. **Empty Input**: Handles empty crate lists gracefully
2. **Single Crate**: Works with single crate scenarios
3. **Circular Dependencies**: Handles circular dependencies gracefully
4. **Self-Dependencies**: Handles crates that depend on themselves
5. **Large Batch Sizes**: Works when batch size exceeds total crates
6. **Mixed Publish Flags**: Correctly filters crates by publish flag

## Mock Objects

### MockCrate
```rust
let crate_info = MockCrate::new("test-crate", "1.0.0")
    .with_dependencies(vec!["dep1", "dep2"])
    .with_publish(false);
```

### MockWorkspace
```rust
let mut workspace = MockWorkspace::new();
workspace.add_crate(crate_a);
workspace.add_crate(crate_b);
let deps = workspace.get_dependencies("crate-b");
```

## Continuous Integration

The test suite is designed to run in CI environments:

- **Fast Execution**: Tests complete in under 10 seconds
- **No External Dependencies**: All tests use mock objects
- **Deterministic**: Tests produce consistent results
- **Comprehensive Coverage**: Covers all major code paths

## Adding New Tests

### For New Functionality
1. Add unit tests in the appropriate module
2. Add integration tests in `tests/integration_tests.rs`
3. Add specific tests in `tests/dependency_batching_tests.rs` if relevant

### For New Scenarios
1. Add scenario function in `tests/test_helpers.rs::scenarios`
2. Add corresponding test in `tests/dependency_batching_tests.rs`
3. Add assertions if needed in `tests/test_helpers.rs::assertions`

### For Performance Testing
1. Add benchmark in `tests/test_helpers.rs::performance`
2. Use `measure_time` for individual measurements
3. Use `benchmark_batching` for scenario comparisons

## Test Maintenance

- **Keep tests focused**: Each test should verify one specific behavior
- **Use descriptive names**: Test names should clearly indicate what they test
- **Avoid test interdependence**: Tests should not depend on each other
- **Mock external dependencies**: Use mock objects instead of real external services
- **Update tests with code changes**: Ensure tests reflect current functionality

## Troubleshooting

### Common Issues

1. **Test failures due to dependency ordering**: Check that the simulation function correctly handles dependencies
2. **Mock object issues**: Ensure mock objects implement required traits
3. **Performance test failures**: Check that benchmarks are deterministic

### Debugging Tests

```bash
# Run with output
cargo test test_name -- --nocapture

# Run with backtrace
RUST_BACKTRACE=1 cargo test test_name

# Run specific test suite
cargo test --test dependency_batching_tests
```

## Future Enhancements

1. **Property-based testing**: Use `proptest` for randomized test inputs
2. **Fuzzing**: Add fuzz testing for edge cases
3. **Coverage reporting**: Add code coverage metrics
4. **Performance regression testing**: Track performance over time
5. **Integration with real Cargo**: Test with actual Cargo workspaces
