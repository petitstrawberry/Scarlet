This is the repository for the Scarlet project. 
which implements a transparent ABI conversion layer for executing binaries across different operating systems and architectures.

## Code Standards

### Required Before Each Commit
- Ensure all tests pass before committing changes.

### Development Flow
Use the docker container of ` scarlet-dev` for development to ensure a consistent environment.
If the image is not available, you can build it using the provided Dockerfile in the repository.

Executing commands in the Docker container at the root of the repository:
- Build: `cargo make build`

❗️ If a cargo make command fails, it's likely because you're not in the container. In that case, run the command from your host machine like this: 
- `docker run -it --rm -v $(pwd):/workspaces/Scarlet scarlet-dev cargo make build`.

#### Testing
Use `cargo make test` to run all tests at the root of the repository.
You cannot run a specific test directly; instead, you can run the entire test suite.
Ensure all tests pass before committing changes to maintain code integrity.

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