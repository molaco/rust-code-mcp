# Project Instructions

- Use the `cuda-code` nix dev shell for project commands:
  `nix develop ../nix-devshells#cuda-code --command {command}`
- Use jujutsu first for VCS operations; fall back to git if jujutsu is unavailable.
- Do not run `cargo fmt` or any formatting command.
- Run build commands through the nix dev shell above.
