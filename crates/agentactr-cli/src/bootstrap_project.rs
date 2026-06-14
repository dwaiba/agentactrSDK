use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

pub(crate) const BOOTSTRAP_STACK_VALUES: &[&str] = &[
    "python",
    "golang",
    "go",
    "rust",
    "typescript",
    "pulumi",
    "terraform",
    "sql",
];

#[derive(Clone, Debug)]
struct ScaffoldFile {
    path: String,
    contents: String,
}

#[derive(Clone, Debug)]
struct BootstrapScaffold {
    stack: String,
    files: Vec<ScaffoldFile>,
    start_commands: Vec<&'static str>,
}

pub(crate) fn cmd_bootstrap(args: &[String]) -> Result<(), String> {
    validate_bootstrap_project_args(args)?;
    if !has_flag(args, "--yes") {
        return Err("bootstrap project is write-capable; pass --yes to scaffold files".to_string());
    }
    let stack = flag_value(args, "--stack").ok_or("missing --stack")?;
    let force = has_flag(args, "--force");
    let allow_non_empty = has_flag(args, "--allow-non-empty");
    let root = env::current_dir().map_err(|e| format!("read current directory: {e}"))?;
    let project_name = project_name_from_root(&root);
    let scaffold = bootstrap_scaffold(&stack, &project_name)?;
    let written = write_bootstrap_scaffold(&root, &scaffold, force, allow_non_empty)?;
    println!("bootstrap_stack={}", scaffold.stack);
    println!("project_name={project_name}");
    println!("written_files={}", written.len());
    for path in &written {
        println!("wrote_file={path}");
    }
    println!("start_commands:");
    for command in &scaffold.start_commands {
        println!("  {command}");
    }
    Ok(())
}

const BOOTSTRAP_PROJECT_USAGE: &str =
    "usage: agentactr bootstrap project --stack python|golang|rust|typescript|pulumi|terraform|sql --yes [--force] [--allow-non-empty]";
const RUST_SCAFFOLD_TOOLCHAIN: &str = "1.95.0";
const RUST_SCAFFOLD_MSRV: &str = "1.95";
const BOOTSTRAP_PROJECT_BOOL_FLAGS: &[&str] = &["--yes", "--force", "--allow-non-empty"];
const BOOTSTRAP_PROJECT_VALUE_FLAGS: &[&str] = &["--stack"];

fn validate_bootstrap_project_args(args: &[String]) -> Result<(), String> {
    if args.get(1).map(String::as_str) != Some("project") {
        return Err(BOOTSTRAP_PROJECT_USAGE.to_string());
    }
    let mut index = 2;
    while index < args.len() {
        let arg = &args[index];
        if BOOTSTRAP_PROJECT_BOOL_FLAGS.contains(&arg.as_str()) {
            index += 1;
            continue;
        }
        if BOOTSTRAP_PROJECT_VALUE_FLAGS.contains(&arg.as_str()) {
            let Some(value) = args.get(index + 1) else {
                return Err(format!("{arg} requires a value; {BOOTSTRAP_PROJECT_USAGE}"));
            };
            if value.starts_with("--") {
                return Err(format!(
                    "{arg} requires a value, got flag `{value}`; {BOOTSTRAP_PROJECT_USAGE}"
                ));
            }
            index += 2;
            continue;
        }
        if arg.starts_with("--") {
            return Err(format!(
                "unknown agentactr bootstrap project flag `{arg}`; {BOOTSTRAP_PROJECT_USAGE}"
            ));
        }
        return Err(format!(
            "unexpected agentactr bootstrap project argument `{arg}`; {BOOTSTRAP_PROJECT_USAGE}"
        ));
    }
    Ok(())
}

fn project_name_from_root(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(normalize_project_slug)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "app".to_string())
}

fn normalize_project_slug(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let normalized = out.trim_matches('-').to_string();
    if normalized
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        normalized
    } else if normalized.is_empty() {
        "app".to_string()
    } else {
        format!("app-{normalized}")
    }
}

fn python_module_name(project_name: &str) -> String {
    project_name.replace('-', "_")
}

fn scaffold_file(path: impl Into<String>, contents: impl Into<String>) -> ScaffoldFile {
    ScaffoldFile {
        path: path.into(),
        contents: contents.into(),
    }
}

fn write_bootstrap_scaffold(
    root: &Path,
    scaffold: &BootstrapScaffold,
    force: bool,
    allow_non_empty: bool,
) -> Result<Vec<String>, String> {
    if !allow_non_empty {
        let non_bootstrap_entries = non_bootstrap_directory_entries(root)?;
        if !non_bootstrap_entries.is_empty() {
            return Err(format!(
                "bootstrap project requires an empty directory; found {}. Rerun with --allow-non-empty only after reviewing the target directory.",
                non_bootstrap_entries.join(", ")
            ));
        }
    }
    let conflicts = scaffold
        .files
        .iter()
        .filter(|file| file.path != ".gitignore" && root.join(&file.path).exists() && !force)
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    if !conflicts.is_empty() {
        return Err(format!(
            "bootstrap target files already exist; rerun with --force to overwrite: {}",
            conflicts.join(", ")
        ));
    }
    let mut written = Vec::new();
    for file in &scaffold.files {
        let target = root.join(&file.path);
        if let Some(parent) = target.parent() {
            create_dir(parent)?;
        }
        if file.path == ".gitignore" && target.exists() && !force {
            merge_scaffold_gitignore(&target, &file.contents)?;
            written.push(file.path.clone());
            continue;
        }
        write_file(&target, &file.contents)?;
        written.push(file.path.clone());
    }
    Ok(written)
}

fn merge_scaffold_gitignore(target: &Path, scaffold_contents: &str) -> Result<(), String> {
    let existing = fs::read_to_string(target)
        .map_err(|e| format!("read existing {}: {e}", target.display()))?;
    let mut merged = existing.clone();
    if !merged.ends_with('\n') {
        merged.push('\n');
    }
    let existing_lines = existing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    for line in scaffold_contents.lines().map(str::trim) {
        if line.is_empty() || existing_lines.contains(&line) {
            continue;
        }
        merged.push_str(line);
        merged.push('\n');
    }
    if merged != existing {
        write_file(target, &merged)?;
    }
    Ok(())
}

fn non_bootstrap_directory_entries(root: &Path) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(root).map_err(|e| format!("read {}: {e}", root.display()))?;
    let mut non_bootstrap = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read {} entry: {e}", root.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if is_agentactr_bootstrap_entry(&name) {
            continue;
        }
        non_bootstrap.push(name);
    }
    non_bootstrap.sort();
    Ok(non_bootstrap)
}

fn is_agentactr_bootstrap_entry(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".agentactr" | ".codex" | ".gitignore" | "agentactr.toml" | "WORKFLOW.md"
    )
}

fn bootstrap_scaffold(stack: &str, project_name: &str) -> Result<BootstrapScaffold, String> {
    match stack.trim().to_ascii_lowercase().as_str() {
        "python" => Ok(python_scaffold(project_name)),
        "golang" | "go" => Ok(golang_scaffold(project_name)),
        "rust" => Ok(rust_scaffold(project_name)),
        "typescript" => Ok(typescript_scaffold(project_name)),
        "pulumi" => Ok(pulumi_scaffold(project_name)),
        "terraform" => Ok(terraform_scaffold(project_name)),
        "sql" => Ok(sql_scaffold(project_name)),
        other => Err(format!(
            "unsupported bootstrap stack `{other}`; expected python|golang|rust|typescript|pulumi|terraform|sql"
        )),
    }
}

fn python_scaffold(project_name: &str) -> BootstrapScaffold {
    let module = python_module_name(project_name);
    BootstrapScaffold {
        stack: "python".to_string(),
        files: vec![
            scaffold_file(".python-version", "3.14.5\n"),
            scaffold_file(
                "poetry.toml",
                "[virtualenvs]\nin-project = true\n\n[installer]\nparallel = true\n",
            ),
            scaffold_file(
                "pyproject.toml",
                format!(
                    r#"[build-system]
requires = ["hatchling>=1.27"]
build-backend = "hatchling.build"

[project]
name = "{project_name}"
version = "0.1.0"
description = "TODO"
readme = "README.md"
requires-python = ">=3.14"
dependencies = []

[dependency-groups]
dev = ["pytest>=8", "ruff>=0.14", "pyright>=1.1"]

[tool.hatch.build.targets.wheel]
packages = ["src/{module}"]

[tool.ruff]
target-version = "py314"
line-length = 100
src = ["src", "tests"]

[tool.ruff.lint]
select = ["E", "F", "I", "B", "UP", "SIM"]

[tool.pytest.ini_options]
testpaths = ["tests"]

[tool.pyright]
include = ["src", "tests"]
pythonVersion = "3.14"
typeCheckingMode = "strict"
"#
                ),
            ),
            scaffold_file(
                ".pre-commit-config.yaml",
                r#"repos:
  - repo: local
    hooks:
      - id: ruff-format
        name: ruff format
        entry: uv run ruff format --check .
        language: system
        pass_filenames: false
      - id: ruff-check
        name: ruff check
        entry: uv run ruff check .
        language: system
        pass_filenames: false
      - id: pyright
        name: pyright
        entry: uv run pyright
        language: system
        pass_filenames: false
      - id: pytest
        name: pytest
        entry: uv run pytest
        language: system
        pass_filenames: false
"#,
            ),
            scaffold_file(
                "README.md",
                format!(
                    "# {project_name}\n\n## Start\n\n```bash\nuv sync\nuv run ruff format --check .\nuv run ruff check .\nuv run pyright\nuv run pytest\n```\n"
                ),
            ),
            scaffold_file(
                ".gitignore",
                ".venv/\n__pycache__/\n.pytest_cache/\n.ruff_cache/\ndist/\n",
            ),
            scaffold_file(format!("src/{module}/__init__.py"), "__version__ = \"0.1.0\"\n"),
            scaffold_file(
                "tests/test_smoke.py",
                format!(
                    "from {module} import __version__\n\n\ndef test_version() -> None:\n    assert __version__ == \"0.1.0\"\n"
                ),
            ),
        ],
        start_commands: vec![
            "uv sync",
            "uv run ruff format --check .",
            "uv run ruff check .",
            "uv run pyright",
            "uv run pytest",
        ],
    }
}

fn golang_scaffold(project_name: &str) -> BootstrapScaffold {
    let module = format!("example.com/{project_name}");
    BootstrapScaffold {
        stack: "golang".to_string(),
        files: vec![
            scaffold_file("go.mod", format!("module {module}\n\ngo 1.26\n")),
            scaffold_file(
                ".golangci.yml",
                "version: \"2\"\nrun:\n  timeout: 5m\nlinters:\n  enable:\n    - govet\n    - ineffassign\n    - staticcheck\n    - unused\n",
            ),
            scaffold_file(
                ".pre-commit-config.yaml",
                r#"repos:
  - repo: local
    hooks:
      - id: gofmt
        name: gofmt
        entry: gofmt -w
        language: system
        types: [go]
      - id: go-mod-tidy-check
        name: go mod tidy check
        entry: bash -lc 'go mod tidy && git diff --exit-code -- go.mod go.sum'
        language: system
        pass_filenames: false
      - id: go-vet
        name: go vet
        entry: go vet ./...
        language: system
        pass_filenames: false
      - id: golangci-lint
        name: golangci-lint
        entry: golangci-lint run
        language: system
        pass_filenames: false
      - id: go-test
        name: go test
        entry: go test ./...
        language: system
        pass_filenames: false
"#,
            ),
            scaffold_file(
                "cmd/app/main.go",
                "package main\n\nimport (\n\t\"fmt\"\n\n\t\"example.com/PROJECT/internal/app\"\n)\n\nfunc main() {\n\tfmt.Println(app.Message())\n}\n"
                    .replace("example.com/PROJECT", &module),
            ),
            scaffold_file(
                "internal/app/app.go",
                "package app\n\nfunc Message() string {\n\treturn \"ok\"\n}\n",
            ),
            scaffold_file(
                "internal/app/app_test.go",
                "package app\n\nimport \"testing\"\n\nfunc TestMessage(t *testing.T) {\n\tif Message() != \"ok\" {\n\t\tt.Fatal(\"unexpected message\")\n\t}\n}\n",
            ),
            scaffold_file(
                "README.md",
                format!("# {project_name}\n\n## Start\n\n```bash\ngo mod tidy\ngofmt -w cmd/app/main.go internal/app/app.go internal/app/app_test.go\ngo vet ./...\ngolangci-lint run\ngo test ./...\ngo run ./cmd/app\n```\n"),
            ),
            scaffold_file(".gitignore", "bin/\ncoverage.out\n"),
        ],
        start_commands: vec![
            "go mod tidy",
            "gofmt -w cmd/app/main.go internal/app/app.go internal/app/app_test.go",
            "go vet ./...",
            "golangci-lint run",
            "go test ./...",
            "go run ./cmd/app",
        ],
    }
}

fn rust_scaffold(project_name: &str) -> BootstrapScaffold {
    let crate_name = project_name.replace('-', "_");
    BootstrapScaffold {
        stack: "rust".to_string(),
        files: vec![
            scaffold_file(
                "rust-toolchain.toml",
                format!(
                    "[toolchain]\nchannel = \"{RUST_SCAFFOLD_TOOLCHAIN}\"\ncomponents = [\"rustfmt\", \"clippy\"]\n"
                ),
            ),
            scaffold_file("Cargo.toml", format!("[workspace]\nmembers = [\"crates/{project_name}-core\", \"crates/{project_name}-cli\"]\nresolver = \"3\"\n\n[workspace.package]\nedition = \"2024\"\nversion = \"0.1.0\"\nlicense = \"Apache-2.0\"\nrust-version = \"{RUST_SCAFFOLD_MSRV}\"\n")),
            scaffold_file(format!("crates/{project_name}-core/Cargo.toml"), format!("[package]\nname = \"{project_name}-core\"\nversion.workspace = true\nedition.workspace = true\nlicense.workspace = true\nrust-version.workspace = true\n\n[lib]\nname = \"{crate_name}_core\"\n")),
            scaffold_file(format!("crates/{project_name}-core/src/lib.rs"), "pub fn message() -> &'static str {\n    \"ok\"\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn message_is_stable() {\n        assert_eq!(crate::message(), \"ok\");\n    }\n}\n"),
            scaffold_file(format!("crates/{project_name}-cli/Cargo.toml"), format!("[package]\nname = \"{project_name}\"\nversion.workspace = true\nedition.workspace = true\nlicense.workspace = true\nrust-version.workspace = true\n\n[dependencies]\n{project_name}-core = {{ path = \"../{project_name}-core\" }}\n")),
            scaffold_file(format!("crates/{project_name}-cli/src/main.rs"), format!("fn main() {{\n    println!(\"{{}}\", {crate_name}_core::message());\n}}\n")),
            scaffold_file("deny.toml", "[advisories]\nversion = 2\n\n[licenses]\nversion = 2\nallow = [\"Apache-2.0\", \"MIT\", \"BSD-2-Clause\", \"BSD-3-Clause\", \"Unicode-3.0\"]\nconfidence-threshold = 0.8\n\n[bans]\nmultiple-versions = \"warn\"\n"),
            scaffold_file(".pre-commit-config.yaml", "repos:\n  - repo: local\n    hooks:\n      - id: cargo-fmt\n        name: cargo fmt\n        entry: cargo fmt --all -- --check\n        language: system\n        pass_filenames: false\n      - id: cargo-clippy\n        name: cargo clippy\n        entry: cargo clippy --workspace --all-targets --all-features -- -D warnings\n        language: system\n        pass_filenames: false\n      - id: cargo-nextest\n        name: cargo nextest\n        entry: cargo nextest run --workspace --all-features\n        language: system\n        pass_filenames: false\n      - id: cargo-deny\n        name: cargo deny\n        entry: cargo deny check\n        language: system\n        pass_filenames: false\n      - id: cargo-machete\n        name: cargo machete\n        entry: cargo machete\n        language: system\n        pass_filenames: false\n"),
            scaffold_file("README.md", format!("# {project_name}\n\nThis scaffold pins Rust {RUST_SCAFFOLD_TOOLCHAIN} in `rust-toolchain.toml` and declares workspace MSRV {RUST_SCAFFOLD_MSRV} in `Cargo.toml`.\n\n## Start\n\n```bash\ncargo fmt --all -- --check\ncargo clippy --workspace --all-targets --all-features -- -D warnings\ncargo nextest run --workspace --all-features\ncargo deny check\ncargo machete\ncargo run -p {project_name}\n```\n")),
            scaffold_file(".gitignore", "target/\n"),
        ],
        start_commands: vec![
            "cargo fmt --all -- --check",
            "cargo clippy --workspace --all-targets --all-features -- -D warnings",
            "cargo nextest run --workspace --all-features",
            "cargo deny check",
            "cargo machete",
        ],
    }
}

fn typescript_scaffold(project_name: &str) -> BootstrapScaffold {
    BootstrapScaffold {
        stack: "typescript".to_string(),
        files: vec![
            scaffold_file(
                "package.json",
                format!(
                    r#"{{
  "name": "{project_name}",
  "version": "0.1.0",
  "type": "module",
  "private": true,
  "packageManager": "bun@1.3.13",
  "scripts": {{
    "format": "biome format --write .",
    "lint": "biome check .",
    "typecheck": "tsc --noEmit",
    "test": "bun test",
    "build": "tsc -p tsconfig.json"
  }},
  "devDependencies": {{
    "@biomejs/biome": "^2.2.0",
    "@types/bun": "^1.3.0",
    "typescript": "^5.9.0"
  }}
}}
"#
                ),
            ),
            scaffold_file(
                "tsconfig.json",
                r#"{
  "compilerOptions": {
    "target": "ES2024",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "exactOptionalPropertyTypes": true,
    "declaration": true,
    "outDir": "dist"
  },
  "include": ["src/**/*.ts", "tests/**/*.ts"]
}
"#,
            ),
            scaffold_file(
                "biome.json",
                r#"{
  "$schema": "https://biomejs.dev/schemas/2.2.0/schema.json",
  "formatter": { "enabled": true },
  "linter": { "enabled": true, "rules": { "recommended": true } },
  "javascript": { "formatter": { "quoteStyle": "double" } }
}
"#,
            ),
            scaffold_file(".pre-commit-config.yaml", "repos:\n  - repo: local\n    hooks:\n      - id: biome\n        name: biome check\n        entry: bunx --bun biome check .\n        language: system\n        pass_filenames: false\n      - id: typecheck\n        name: typecheck\n        entry: bun run typecheck\n        language: system\n        pass_filenames: false\n      - id: test\n        name: bun test\n        entry: bun test\n        language: system\n        pass_filenames: false\n"),
            scaffold_file(
                "src/index.ts",
                "export function message(): string {\n  return \"ok\";\n}\n",
            ),
            scaffold_file(
                "tests/index.test.ts",
                "import { expect, test } from \"bun:test\";\nimport { message } from \"../src/index.js\";\n\ntest(\"message\", () => {\n  expect(message()).toBe(\"ok\");\n});\n",
            ),
            scaffold_file(
                "README.md",
                format!("# {project_name}\n\n## Start\n\n```bash\nbun install\nbunx skills add pbakaus/impeccable\nbunx skills add https://github.com/magicpathai/agent-skills --skill magicpath\nbun run lint\nbun run typecheck\nbun test\nbun run build\n```\n"),
            ),
            scaffold_file(".gitignore", "node_modules/\ndist/\ncoverage/\n"),
        ],
        start_commands: vec![
            "bun install",
            "bunx skills add pbakaus/impeccable",
            "bunx skills add https://github.com/magicpathai/agent-skills --skill magicpath",
            "bun run lint",
            "bun run typecheck",
            "bun test",
            "bun run build",
        ],
    }
}

fn pulumi_scaffold(project_name: &str) -> BootstrapScaffold {
    BootstrapScaffold {
        stack: "pulumi".to_string(),
        files: vec![
            scaffold_file(
                "Pulumi.yaml",
                format!(
                    "name: {project_name}\nruntime: nodejs\ndescription: Modular Pulumi TypeScript project\n"
                ),
            ),
            scaffold_file(
                "package.json",
                format!(
                    r#"{{
  "name": "{project_name}",
  "version": "0.1.0",
  "type": "module",
  "private": true,
  "packageManager": "bun@1.3.13",
  "scripts": {{
    "lint": "biome check .",
    "typecheck": "tsc --noEmit",
    "test": "bun test"
  }},
  "dependencies": {{
    "@pulumi/pulumi": "^3.0.0"
  }},
  "devDependencies": {{
    "@biomejs/biome": "^2.2.0",
    "@types/bun": "^1.3.0",
    "typescript": "^5.9.0"
  }}
}}
"#
                ),
            ),
            scaffold_file(
                "tsconfig.json",
                r#"{
  "compilerOptions": {
    "target": "ES2024",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "noUncheckedIndexedAccess": true
  },
  "include": ["src/**/*.ts", "tests/**/*.ts"]
}
"#,
            ),
            scaffold_file("biome.json", r#"{"$schema":"https://biomejs.dev/schemas/2.2.0/schema.json","formatter":{"enabled":true},"linter":{"enabled":true,"rules":{"recommended":true}}}
"#),
            scaffold_file("src/index.ts", "import * as pulumi from \"@pulumi/pulumi\";\n\nconst config = new pulumi.Config();\nexport const environment = config.get(\"environment\") ?? \"dev\";\n"),
            scaffold_file("tests/config.test.ts", "import { expect, test } from \"bun:test\";\n\ntest(\"placeholder\", () => {\n  expect(true).toBe(true);\n});\n"),
            scaffold_file(".pre-commit-config.yaml", "repos:\n  - repo: local\n    hooks:\n      - id: biome\n        name: biome check\n        entry: bunx --bun biome check .\n        language: system\n        pass_filenames: false\n      - id: typecheck\n        name: typecheck\n        entry: bun run typecheck\n        language: system\n        pass_filenames: false\n      - id: test\n        name: bun test\n        entry: bun test\n        language: system\n        pass_filenames: false\n"),
            scaffold_file("README.md", format!("# {project_name}\n\n## Start\n\n```bash\nbun install\nbun run lint\nbun run typecheck\nbun test\n```\n\n## Optional Pulumi Preview\n\nPulumi preview can require credentials, backend access, and network. Run it only after selecting a Pulumi backend and reviewing the target stack.\n\n```bash\npulumi stack init dev\npulumi config set environment dev\npulumi preview --non-interactive --diff\n```\n")),
            scaffold_file(".gitignore", "node_modules/\ndist/\n.pulumi/\n"),
        ],
        start_commands: vec![
            "bun install",
            "bun run lint",
            "bun run typecheck",
            "bun test",
        ],
    }
}

fn terraform_scaffold(project_name: &str) -> BootstrapScaffold {
    BootstrapScaffold {
        stack: "terraform".to_string(),
        files: vec![
            scaffold_file(
                "versions.tf",
                "terraform {\n  required_version = \">= 1.14.0, < 2.0.0\"\n}\n",
            ),
            scaffold_file("variables.tf", "variable \"name\" {\n  type        = string\n  description = \"Logical name for this stack.\"\n  default     = \"example\"\n}\n"),
            scaffold_file("main.tf", "module \"example\" {\n  source = \"./modules/example\"\n  name   = var.name\n}\n"),
            scaffold_file("outputs.tf", "output \"name\" {\n  value = module.example.name\n}\n"),
            scaffold_file("modules/example/variables.tf", "variable \"name\" {\n  type = string\n}\n"),
            scaffold_file("modules/example/main.tf", "locals {\n  normalized_name = lower(trimspace(var.name))\n}\n"),
            scaffold_file("modules/example/outputs.tf", "output \"name\" {\n  value = local.normalized_name\n}\n"),
            scaffold_file("tests/main.tftest.hcl", "run \"name_is_normalized\" {\n  command = plan\n\n  variables {\n    name = \"Example\"\n  }\n\n  assert {\n    condition     = output.name == \"example\"\n    error_message = \"name should be normalized\"\n  }\n}\n"),
            scaffold_file(".pre-commit-config.yaml", "repos:\n  - repo: local\n    hooks:\n      - id: terraform-fmt\n        name: terraform fmt\n        entry: terraform fmt -check -recursive\n        language: system\n        pass_filenames: false\n      - id: terraform-init-validate\n        name: terraform validate\n        entry: bash -lc 'terraform init -backend=false -lockfile=readonly && terraform validate'\n        language: system\n        pass_filenames: false\n      - id: terraform-test\n        name: terraform test\n        entry: terraform test\n        language: system\n        pass_filenames: false\n"),
            scaffold_file("README.md", format!("# {project_name}\n\n## Start\n\n```bash\nterraform fmt -recursive\nterraform init -backend=false\nterraform validate\nterraform test\nterraform plan\n```\n\nCommit `.terraform.lock.hcl` after provider selection so fresh checkouts can run readonly lockfile validation reproducibly.\n")),
            scaffold_file(".gitignore", ".terraform/\n*.tfstate\n*.tfstate.*\n# Keep .terraform.lock.hcl tracked for reproducible provider selections.\n"),
        ],
        start_commands: vec![
            "terraform fmt -recursive",
            "terraform init -backend=false",
            "terraform validate",
            "terraform test",
            "terraform plan",
        ],
    }
}

fn sql_scaffold(project_name: &str) -> BootstrapScaffold {
    BootstrapScaffold {
        stack: "sql".to_string(),
        files: vec![
            scaffold_file("migrations/0001_init.up.sql", "-- Forward-only schema migration.\nCREATE TABLE IF NOT EXISTS schema_migrations (\n    version TEXT PRIMARY KEY,\n    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\n);\n"),
            scaffold_file("rollbacks/0001_init.down.sql", "-- Reviewed rollback for 0001_init.up.sql.\nDROP TABLE IF EXISTS schema_migrations;\n"),
            scaffold_file("backfills/README.md", "# Backfills\n\nBackfills must be idempotent, batched, observable, and reviewed separately from schema migrations.\n"),
            scaffold_file("backfills/0001_example_backfill.sql", "-- Idempotent backfill template.\n-- UPDATE target_table SET column_name = value WHERE column_name IS NULL;\n"),
            scaffold_file("seeds/README.md", "# Seeds\n\nKeep deterministic local/dev seed data here. Do not include production data or secrets.\n"),
            scaffold_file("tests/0001_schema_smoke.sql", "-- Add database-specific smoke assertions here.\nSELECT 1;\n"),
            scaffold_file(".sqlfluff", "[sqlfluff]\ndialect = postgres\ntemplater = raw\nmax_line_length = 100\n"),
            scaffold_file(".pre-commit-config.yaml", "repos:\n  - repo: local\n    hooks:\n      - id: sqlfluff-lint\n        name: sqlfluff lint\n        entry: sqlfluff lint migrations backfills tests\n        language: system\n        pass_filenames: false\n"),
            scaffold_file("README.md", format!("# {project_name}\n\n## Start\n\n```bash\nsqlfluff lint migrations backfills tests\n# Apply migrations with your reviewed migration runner.\n# Run backfills separately with batching, observability, and rollback notes.\n```\n")),
        ],
        start_commands: vec![
            "sqlfluff lint migrations backfills tests",
            "review migrations/ and rollbacks/ before applying",
            "run backfills separately with batching and observability",
        ],
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].clone())
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn create_dir(path: impl AsRef<Path>) -> Result<(), String> {
    fs::create_dir_all(path.as_ref())
        .map_err(|e| format!("create directory {}: {e}", path.as_ref().display()))
}

fn write_file(path: impl AsRef<Path>, content: &str) -> Result<(), String> {
    let path = path.as_ref();
    let mut file = fs::File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bootstrap_project_scaffolds_all_supported_stacks_without_overwrite() {
        for stack in [
            "python",
            "golang",
            "rust",
            "typescript",
            "pulumi",
            "terraform",
            "sql",
        ] {
            let root = temp_root(&format!("agentactr-bootstrap-{stack}"));
            fs::create_dir_all(&root).unwrap();
            let scaffold = bootstrap_scaffold(stack, "demo-project").unwrap();
            let written = write_bootstrap_scaffold(&root, &scaffold, false, false).unwrap();
            assert!(!written.is_empty(), "no files written for {stack}");
            assert!(
                root.join("README.md").exists(),
                "README missing for {stack}"
            );
            assert!(
                root.join(".pre-commit-config.yaml").exists(),
                "pre-commit missing for {stack}"
            );
            let err = write_bootstrap_scaffold(&root, &scaffold, false, true).unwrap_err();
            assert!(err.contains("--force"));
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn bootstrap_project_refuses_non_empty_directory_without_explicit_allowance() {
        let root = temp_root("agentactr-bootstrap-non-empty");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("existing.txt"), "operator data\n").unwrap();
        let scaffold = bootstrap_scaffold("python", "demo-project").unwrap();

        let err = write_bootstrap_scaffold(&root, &scaffold, false, false).unwrap_err();

        assert!(err.contains("requires an empty directory"));
        assert!(err.contains("--allow-non-empty"));
        assert!(!root.join("pyproject.toml").exists());
        let written = write_bootstrap_scaffold(&root, &scaffold, false, true).unwrap();
        assert!(written.iter().any(|path| path == "pyproject.toml"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bootstrap_project_allows_agentactr_init_metadata_in_blank_project() {
        let root = temp_root("agentactr-bootstrap-init-metadata");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::create_dir_all(root.join(".agentactr")).unwrap();
        fs::write(root.join("agentactr.toml"), "[tracker]\n").unwrap();
        fs::write(root.join("WORKFLOW.md"), "# Workflow\n").unwrap();
        fs::write(root.join(".gitignore"), ".agentactr/\n").unwrap();
        let scaffold = bootstrap_scaffold("python", "demo-project").unwrap();

        let written = write_bootstrap_scaffold(&root, &scaffold, false, false).unwrap();

        assert!(written.iter().any(|path| path == "pyproject.toml"));
        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".agentactr/"));
        assert!(gitignore.contains(".venv/"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pulumi_scaffold_keeps_live_preview_out_of_default_hooks_and_start_commands() {
        let scaffold = pulumi_scaffold("demo-project");
        let pre_commit = scaffold
            .files
            .iter()
            .find(|file| file.path == ".pre-commit-config.yaml")
            .unwrap();
        let package_json = scaffold
            .files
            .iter()
            .find(|file| file.path == "package.json")
            .unwrap();
        let readme = scaffold
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .unwrap();

        assert!(!pre_commit.contents.contains("pulumi preview"));
        assert!(!package_json.contents.contains("\"preview\""));
        assert!(!scaffold
            .start_commands
            .iter()
            .any(|command| command.contains("pulumi preview")));
        assert!(readme.contents.contains("Optional Pulumi Preview"));
        assert!(readme.contents.contains("credential"));
    }

    #[test]
    fn rust_scaffold_pins_toolchain_and_workspace_msrv() {
        let scaffold = rust_scaffold("demo-project");
        let toolchain = scaffold
            .files
            .iter()
            .find(|file| file.path == "rust-toolchain.toml")
            .unwrap();
        let workspace_manifest = scaffold
            .files
            .iter()
            .find(|file| file.path == "Cargo.toml")
            .unwrap();
        let core_manifest = scaffold
            .files
            .iter()
            .find(|file| file.path == "crates/demo-project-core/Cargo.toml")
            .unwrap();
        let cli_manifest = scaffold
            .files
            .iter()
            .find(|file| file.path == "crates/demo-project-cli/Cargo.toml")
            .unwrap();
        let readme = scaffold
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .unwrap();

        assert!(toolchain
            .contents
            .contains(&format!("channel = \"{RUST_SCAFFOLD_TOOLCHAIN}\"")));
        assert!(!toolchain.contents.contains("channel = \"stable\""));
        assert!(workspace_manifest
            .contents
            .contains(&format!("rust-version = \"{RUST_SCAFFOLD_MSRV}\"")));
        assert!(core_manifest
            .contents
            .contains("rust-version.workspace = true"));
        assert!(cli_manifest
            .contents
            .contains("rust-version.workspace = true"));
        assert!(readme.contents.contains("pins Rust"));
        assert!(readme.contents.contains("workspace MSRV"));
    }

    #[test]
    fn terraform_scaffold_keeps_provider_lock_tracked() {
        let scaffold = terraform_scaffold("demo-project");
        let gitignore = scaffold
            .files
            .iter()
            .find(|file| file.path == ".gitignore")
            .unwrap();
        let pre_commit = scaffold
            .files
            .iter()
            .find(|file| file.path == ".pre-commit-config.yaml")
            .unwrap();
        let readme = scaffold
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .unwrap();

        assert!(gitignore.contents.contains(".terraform/"));
        assert!(gitignore.contents.contains("*.tfstate"));
        assert!(!gitignore
            .contents
            .lines()
            .any(|line| line.trim() == ".terraform.lock.hcl"));
        assert!(gitignore
            .contents
            .contains("Keep .terraform.lock.hcl tracked"));
        assert!(pre_commit.contents.contains("-lockfile=readonly"));
        assert!(readme.contents.contains("Commit `.terraform.lock.hcl`"));
    }

    #[test]
    fn bootstrap_project_slug_is_safe_for_language_identifiers() {
        assert_eq!(normalize_project_slug("123 API"), "app-123-api");
        assert_eq!(normalize_project_slug("!!!"), "app");

        let python = python_scaffold("app-123-api");
        assert!(python
            .files
            .iter()
            .any(|file| file.path == "src/app_123_api/__init__.py"));
        assert!(python.files.iter().any(|file| file
            .contents
            .contains("from app_123_api import __version__")));

        let rust = rust_scaffold("app-123-api");
        assert!(rust
            .files
            .iter()
            .any(|file| file.contents.contains("name = \"app_123_api_core\"")));
        assert!(rust
            .files
            .iter()
            .any(|file| file.contents.contains("app_123_api_core::message()")));
    }

    #[test]
    fn golang_scaffold_prints_gofmt_compatible_paths() {
        let scaffold = golang_scaffold("demo-project");
        let readme = scaffold
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .unwrap();

        assert!(!readme.contents.contains("gofmt -w ./..."));
        assert!(!scaffold.start_commands.contains(&"gofmt -w ./..."));
        assert!(readme
            .contents
            .contains("gofmt -w cmd/app/main.go internal/app/app.go internal/app/app_test.go"));
        assert!(scaffold
            .start_commands
            .contains(&"gofmt -w cmd/app/main.go internal/app/app.go internal/app/app_test.go"));
    }

    #[test]
    fn bun_scaffolds_use_fresh_install_and_nodenext_test_imports() {
        let typescript = typescript_scaffold("demo-project");
        let ts_test = typescript
            .files
            .iter()
            .find(|file| file.path == "tests/index.test.ts")
            .unwrap();
        let ts_readme = typescript
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .unwrap();

        assert!(ts_test.contents.contains("from \"../src/index.js\""));
        assert!(ts_readme.contents.contains("bun install\n"));
        assert!(ts_readme
            .contents
            .contains("bunx skills add pbakaus/impeccable"));
        assert!(ts_readme.contents.contains(
            "bunx skills add https://github.com/magicpathai/agent-skills --skill magicpath"
        ));
        assert!(!ts_readme.contents.contains("--frozen-lockfile"));
        assert!(typescript.start_commands.contains(&"bun install"));
        assert!(typescript
            .start_commands
            .contains(&"bunx skills add pbakaus/impeccable"));
        assert!(typescript.start_commands.contains(
            &"bunx skills add https://github.com/magicpathai/agent-skills --skill magicpath"
        ));
        assert!(!typescript
            .start_commands
            .iter()
            .any(|command| command.contains("--frozen-lockfile")));

        let pulumi = pulumi_scaffold("demo-project");
        let pulumi_readme = pulumi
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .unwrap();
        assert!(pulumi_readme.contents.contains("bun install\n"));
        assert!(!pulumi_readme.contents.contains("--frozen-lockfile"));
        assert!(pulumi.start_commands.contains(&"bun install"));
        assert!(!pulumi
            .start_commands
            .iter()
            .any(|command| command.contains("--frozen-lockfile")));
    }

    #[test]
    fn bootstrap_project_args_reject_unknown_flags_before_writes() {
        let args = vec![
            "bootstrap".to_string(),
            "project".to_string(),
            "--stack".to_string(),
            "rust".to_string(),
            "--yes".to_string(),
            "--bogus".to_string(),
        ];

        let err = validate_bootstrap_project_args(&args).unwrap_err();

        assert!(err.contains("unknown agentactr bootstrap project flag `--bogus`"));
    }

    #[test]
    fn bootstrap_project_args_reject_missing_or_stray_values() {
        let missing = vec![
            "bootstrap".to_string(),
            "project".to_string(),
            "--stack".to_string(),
            "--yes".to_string(),
        ];
        let err = validate_bootstrap_project_args(&missing).unwrap_err();
        assert!(err.contains("--stack requires a value, got flag `--yes`"));

        let stray = vec![
            "bootstrap".to_string(),
            "project".to_string(),
            "--stack".to_string(),
            "rust".to_string(),
            "--yes".to_string(),
            "extra".to_string(),
        ];
        let err = validate_bootstrap_project_args(&stray).unwrap_err();
        assert!(err.contains("unexpected agentactr bootstrap project argument `extra`"));
    }

    fn temp_root(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("{name}-{}-{suffix}", std::process::id()))
    }
}
