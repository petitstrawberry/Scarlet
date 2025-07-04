This is the repository for the Scarlet project. 
which implements a transparent ABI conversion layer for executing binaries across different operating systems and architectures.

## Code Standards

### Required Before Each Commit
- Run `cargo make test` at the root of the repository to ensure all tests pass.

### Development Flow
Executing commands in the root directory of the repository:
- Build: `cargo make build` 
- Test: `cargo make test`

#### Testing
Use `cargo make test` to run all tests.
You cannot run a specific test directly; instead, you can run the entire test suite.

## Key guidelines
- Use `cargo make` for all commands to ensure consistency.
- Ensure all tests pass before committing changes.
- Follow the existing code style and structure.
- Use descriptive commit messages that explain the changes made.
- Avoid making changes that break existing functionality without providing a clear migration path or explanation.
- Document any new features or changes in the codebase in English. Refer to existing documentation for style and format.
- Comment rust-doc style for public functions and modules to maintain clarity.
- Write tests for new features or changes to ensure they work as expected. This project is `no_std`, so tests should be written in a way that does not rely on the standard library and their tests should be written as `#[test_case]` to ensure compatibility with `no_std`.
- Commit changes frequently to avoid large, unwieldy commits.