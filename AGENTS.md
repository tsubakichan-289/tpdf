# Development

- Use `nix develop` for all development commands, or `nix develop -c <command>`.
- Declare toolchains and native dependencies in `flake.nix`; commit `flake.lock`.
- Do not use global Rust installations, ad-hoc `nix-shell -p`, or hard-coded
  `/nix/store` paths. Preserve lock pins unless an update is requested.
- Mark new flake files with `git add -N -- flake.nix flake.lock` in existing
  Git repositories, preserving already staged content. Verify plain `nix develop`.
- Run relevant checks inside the dev shell and update README setup instructions
  when dependencies change.
