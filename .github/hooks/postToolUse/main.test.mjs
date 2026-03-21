import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const currentFile = fileURLToPath(import.meta.url);
const postToolUseDir = path.dirname(currentFile);
const repoRoot = path.resolve(postToolUseDir, "..", "..", "..");
const hooksConfigPath = path.join(repoRoot, ".github", "hooks", "hooks.json");
const lintersConfigPath = path.join(
	repoRoot,
	".github",
	"hooks",
	"postToolUse",
	"linters.json",
);
const sourcePostToolUseDir = path.join(
	repoRoot,
	".github",
	"hooks",
	"postToolUse",
);

function run(command, args, options = {}) {
	const result = spawnSync(command, args, {
		encoding: "utf8",
		...options,
	});

	if (result.error) {
		throw result.error;
	}

	return result;
}

function writeExecutable(filePath, content) {
	fs.mkdirSync(path.dirname(filePath), { recursive: true });
	fs.writeFileSync(filePath, content, { mode: 0o755 });
}

function setupRepo(t, prefix) {
	const repo = fs.mkdtempSync(path.join(os.tmpdir(), prefix));
	t.after(() => {
		fs.rmSync(repo, { recursive: true, force: true });
	});

	run("git", ["init", "--quiet"], { cwd: repo });
	run("git", ["config", "user.name", "test"], { cwd: repo });
	run("git", ["config", "user.email", "test@example.com"], { cwd: repo });
	fs.mkdirSync(path.join(repo, ".github", "hooks"), { recursive: true });
	fs.cpSync(
		sourcePostToolUseDir,
		path.join(repo, ".github", "hooks", "postToolUse"),
		{
			recursive: true,
		},
	);
	fs.appendFileSync(
		path.join(repo, ".git", "info", "exclude"),
		["bin/", "hook.log", ".github/hooks/postToolUse/"].join("\n") + "\n",
		"utf8",
	);

	return repo;
}

function createToolStubs(
	repo,
	{
		markdownlint = true,
		biome = true,
		oxlint = true,
		eslint = false,
		ruff = false,
		podman = false,
		hadolint = false,
		firstTool = false,
		secondTool = false,
	} = {},
) {
	const binDir = path.join(repo, "bin");
	const logFile = path.join(repo, "hook.log");

	if (markdownlint) {
		writeExecutable(
			path.join(binDir, "markdownlint-cli2"),
			[
				"#!/bin/sh",
				'printf "%s\\n" "$*" >> "$HOOK_LOG"',
				'if [ "$1" = "--fix" ]; then',
				"  exit 0",
				"fi",
				'printf "remaining markdown issue in %s\\n" "$1" >&2',
				"exit 1",
			].join("\n"),
		);
	}

	if (biome) {
		writeExecutable(
			path.join(binDir, "biome"),
			[
				"#!/bin/sh",
				'printf "biome %s\\n" "$*" >> "$HOOK_LOG"',
				'file="$2"',
				'if [ "$2" = "--write" ]; then',
				'  file="$3"',
				"  exit 0",
				"fi",
				'printf "biome unresolved in %s\\n" "$file" >&2',
				"exit 1",
			].join("\n"),
		);
	}

	if (oxlint) {
		writeExecutable(
			path.join(binDir, "oxlint"),
			[
				"#!/bin/sh",
				'printf "oxlint %s\\n" "$*" >> "$HOOK_LOG"',
				'if [ "$1" = "--fix" ]; then',
				"  exit 0",
				"fi",
				'printf "oxlint unresolved in %s\\n" "$1" >&2',
				"exit 1",
			].join("\n"),
		);
	}

	if (eslint) {
		writeExecutable(
			path.join(binDir, "eslint"),
			[
				"#!/bin/sh",
				'printf "eslint %s\\n" "$*" >> "$HOOK_LOG"',
				'if [ "$1" = "--fix" ]; then',
				"  exit 0",
				"fi",
				'printf "eslint unresolved in %s\\n" "$1" >&2',
				"exit 1",
			].join("\n"),
		);
	}

	if (ruff) {
		writeExecutable(
			path.join(binDir, "ruff"),
			[
				"#!/bin/sh",
				'printf "ruff %s\\n" "$*" >> "$HOOK_LOG"',
				'if [ "$1" = "format" ]; then',
				"  exit 0",
				"fi",
				'if [ "$1" = "check" ] && [ "$2" = "--fix" ]; then',
				"  exit 0",
				"fi",
				'printf "ruff unresolved in %s\\n" "$2" >&2',
				"exit 1",
			].join("\n"),
		);
	}

	if (podman) {
		writeExecutable(
			path.join(binDir, "podman"),
			[
				"#!/bin/sh",
				'printf "podman %s\\n" "$*" >> "$HOOK_LOG"',
				'case "$*" in',
				'  *"cp -a /usr/local/rustup/. /host-rustup/ && cp -a /usr/local/cargo/. /host-cargo/"*)',
				"    exit 0 ;;",
				'  *"cargo fmt --all"*)',
				"    exit 0 ;;",
				'  *"cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets"*)',
				"    exit 0 ;;",
				"esac",
				'printf "clippy unresolved\\n" >&2',
				"exit 1",
			].join("\n"),
		);
	}

	if (hadolint) {
		writeExecutable(
			path.join(binDir, "hadolint"),
			[
				"#!/bin/sh",
				'printf "hadolint %s\\n" "$*" >> "$HOOK_LOG"',
				'printf "hadolint unresolved in %s\\n" "$1" >&2',
				"exit 1",
			].join("\n"),
		);
	}

	if (firstTool) {
		writeExecutable(
			path.join(binDir, "first-tool"),
			[
				"#!/bin/sh",
				'printf "first-tool %s\\n" "$*" >> "$HOOK_LOG"',
				"exit 0",
			].join("\n"),
		);
	}

	if (secondTool) {
		writeExecutable(
			path.join(binDir, "second-tool"),
			[
				"#!/bin/sh",
				'printf "second-tool %s\\n" "$*" >> "$HOOK_LOG"',
				"exit 0",
			].join("\n"),
		);
	}

	return {
		env: {
			...process.env,
			PATH: `${binDir}:/usr/bin:/bin:/usr/sbin:/sbin`,
			HOOK_LOG: logFile,
		},
		logFile,
	};
}

function seedRepo(repo) {
	fs.writeFileSync(path.join(repo, "README.md"), "# Title\n", "utf8");
	fs.writeFileSync(
		path.join(repo, "index.ts"),
		"export const value = 1;\n",
		"utf8",
	);
	run("git", ["add", "README.md", "index.ts"], { cwd: repo });
	run("git", ["commit", "-m", "init"], { cwd: repo, stdio: "ignore" });
}

function makeFilesDirty(repo) {
	fs.writeFileSync(
		path.join(repo, "README.md"),
		"# Title\n\nchanged\n",
		"utf8",
	);
	fs.writeFileSync(
		path.join(repo, "index.ts"),
		"export const value=1\n",
		"utf8",
	);
}

function changeDirtyFilesAgain(repo) {
	fs.appendFileSync(path.join(repo, "README.md"), "\nchanged again\n", "utf8");
	fs.appendFileSync(
		path.join(repo, "index.ts"),
		"console.log(value)\n",
		"utf8",
	);
}

function seedPythonRustDockerRepo(repo) {
	fs.mkdirSync(path.join(repo, "src"), { recursive: true });
	fs.writeFileSync(path.join(repo, "app.py"), "value = 1\n", "utf8");
	fs.writeFileSync(
		path.join(repo, "Cargo.toml"),
		[
			"[package]",
			'name = "demo"',
			'version = "0.1.0"',
			'edition = "2021"',
			"",
		].join("\n"),
		"utf8",
	);
	fs.writeFileSync(
		path.join(repo, "src", "lib.rs"),
		"pub fn value() -> i32 { 1 }\n",
		"utf8",
	);
	fs.writeFileSync(
		path.join(repo, "Dockerfile"),
		"FROM alpine:3.20\nRUN echo hello\n",
		"utf8",
	);
	run("git", ["add", "app.py", "Cargo.toml", "src/lib.rs", "Dockerfile"], {
		cwd: repo,
	});
	run("git", ["commit", "-m", "init"], { cwd: repo, stdio: "ignore" });
}

function makePythonRustDockerDirty(repo) {
	fs.writeFileSync(path.join(repo, "app.py"), "value=1\n", "utf8");
	fs.writeFileSync(
		path.join(repo, "src", "lib.rs"),
		"pub fn value()->i32{1}\n",
		"utf8",
	);
	fs.writeFileSync(
		path.join(repo, "Dockerfile"),
		"FROM alpine:3.20\nRUN apk add curl\n",
		"utf8",
	);
}

function runHook(repo, env, toolResultType = "success") {
	return run(process.execPath, [".github/hooks/postToolUse/main.mjs"], {
		cwd: repo,
		env,
		input: JSON.stringify({
			cwd: repo,
			toolName: "bash",
			toolResult: { resultType: toolResultType },
		}),
	});
}

test("hooks config uses one main postToolUse command", () => {
	const hooksConfig = JSON.parse(fs.readFileSync(hooksConfigPath, "utf8"));
	assert.equal(hooksConfig.hooks.postToolUse.length, 1);
	assert.equal(
		hooksConfig.hooks.postToolUse[0].bash,
		"node .github/hooks/postToolUse/main.mjs",
	);
	assert.equal(
		hooksConfig.hooks.postToolUse[0].powershell,
		"node .github/hooks/postToolUse/main.mjs",
	);
	assert.equal(hooksConfig.hooks.postToolUse[0].timeoutSec, 300);
});

test("linters config defines concrete tools and language pipelines", () => {
	const lintersConfig = JSON.parse(fs.readFileSync(lintersConfigPath, "utf8"));
	const markdownlintFixNpx = lintersConfig.tools.find(
		(tool) => tool.id === "markdownlint-fix-npx",
	);
	const cargoFmt = lintersConfig.tools.find((tool) => tool.id === "cargo-fmt");
	const markdownPipeline = lintersConfig.pipelines.find(
		(pipeline) => pipeline.id === "markdown",
	);
	const scriptsPipeline = lintersConfig.pipelines.find(
		(pipeline) => pipeline.id === "scripts",
	);
	const pythonPipeline = lintersConfig.pipelines.find(
		(pipeline) => pipeline.id === "python",
	);
	const rustPipeline = lintersConfig.pipelines.find(
		(pipeline) => pipeline.id === "rust",
	);
	const dockerPipeline = lintersConfig.pipelines.find(
		(pipeline) => pipeline.id === "dockerfile",
	);

	assert.equal(markdownlintFixNpx.command, "npx");
	assert.deepEqual(markdownlintFixNpx.args, [
		"--yes",
		"markdownlint-cli2",
		"--fix",
	]);
	assert.equal(cargoFmt.command, "sh");
	assert.match(cargoFmt.args[1], /podman run --rm -i/);
	assert.match(cargoFmt.args[1], /docker\.io\/rust:1\.94\.0-bookworm/);
	assert.equal(cargoFmt.appendFiles, false);
	assert.equal(markdownPipeline.id, "markdown");
	assert.deepEqual(markdownPipeline.matcher, ["\\.(?:md|markdown)$"]);
	assert.deepEqual(markdownPipeline.steps[0].tools, [
		"markdownlint-fix",
		"markdownlint-fix-npx",
	]);
	assert.equal(scriptsPipeline.id, "scripts");
	assert.deepEqual(scriptsPipeline.matcher, ["\\.(?:[cm]?[jt]s|[jt]sx)$"]);
	assert.deepEqual(scriptsPipeline.steps[1].tools, [
		"oxlint-fix",
		"eslint-fix",
		"eslint-fix-npx",
		"oxlint-fix-npx",
	]);
	assert.deepEqual(pythonPipeline.matcher, ["\\.(?:py|pyi|pyw)$"]);
	assert.deepEqual(rustPipeline.matcher, ["\\.rs$"]);
	assert.deepEqual(dockerPipeline.matcher, [
		"(?:^|/)Dockerfile(?:\\.[^/]+)?$",
		"(?:^|/)[^/]+\\.Dockerfile$",
	]);
	assert.equal(lintersConfig.pipelines.length, 5);
});

test("main hook parses input once and runs configured linters incrementally", (t) => {
	const repo = setupRepo(t, "hook-main-");
	seedRepo(repo);
	makeFilesDirty(repo);

	const { env, logFile } = createToolStubs(repo);

	const firstRun = runHook(repo, env);
	const firstLog = fs.readFileSync(logFile, "utf8");

	const secondRun = runHook(repo, env);
	const secondLog = fs.readFileSync(logFile, "utf8");

	changeDirtyFilesAgain(repo);
	const thirdRun = runHook(repo, env);
	const thirdLog = fs.readFileSync(logFile, "utf8");

	assert.equal(firstRun.status, 1);
	assert.equal(secondRun.status, 0);
	assert.equal(thirdRun.status, 1);

	assert.equal(firstLog.trim().split("\n").length, 7);
	assert.equal(secondLog.trim().split("\n").length, 7);
	assert.equal(thirdLog.trim().split("\n").length, 14);

	assert.match(firstLog, /--fix README\.md/);
	assert.match(firstLog, /biome check --write index\.ts/);
	assert.match(firstLog, /oxlint --fix index\.ts/);

	assert.match(firstRun.stderr, /remaining markdown issue in README\.md/);
	assert.match(firstRun.stderr, /Biome reported unresolved issues:/);
	assert.match(
		firstRun.stderr,
		/JavaScript\/TypeScript linter reported unresolved issues:/,
	);
	assert.equal(secondRun.stderr, "");
	assert.match(thirdRun.stderr, /README\.md/);
	assert.match(thirdRun.stderr, /index\.ts/);
});

test("main hook can fall back from oxlint to eslint", (t) => {
	const repo = setupRepo(t, "hook-eslint-fallback-");
	seedRepo(repo);
	fs.writeFileSync(
		path.join(repo, "index.ts"),
		"export const value=1\n",
		"utf8",
	);

	const { env, logFile } = createToolStubs(repo, {
		oxlint: false,
		eslint: true,
	});
	const result = runHook(repo, env);
	const hookLog = fs.readFileSync(logFile, "utf8");

	assert.equal(result.status, 1);
	assert.equal(hookLog.includes("oxlint --fix index.ts"), false);
	assert.match(hookLog, /eslint --fix index\.ts/);
	assert.match(
		result.stderr,
		/JavaScript\/TypeScript linter reported unresolved issues:/,
	);
	assert.match(result.stderr, /eslint unresolved in index\.ts/);
});

test("main hook runs Python Rust and Dockerfile pipelines", (t) => {
	const repo = setupRepo(t, "hook-extra-languages-");
	seedPythonRustDockerRepo(repo);
	makePythonRustDockerDirty(repo);

	const { env, logFile } = createToolStubs(repo, {
		markdownlint: false,
		biome: false,
		oxlint: false,
		eslint: false,
		ruff: true,
		podman: true,
		hadolint: true,
	});
	const result = runHook(repo, env);
	const hookLog = fs.readFileSync(logFile, "utf8");

	assert.equal(result.status, 1);
	assert.match(hookLog, /ruff format app\.py/);
	assert.match(hookLog, /ruff check --fix app\.py/);
	assert.match(
		hookLog,
		/podman run .*docker\.io\/rust:1\.94\.0-bookworm .*cargo fmt --all/,
	);
	assert.match(
		hookLog,
		/podman run .*cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets/,
	);
	assert.equal(hookLog.includes("cargo fmt --all src/lib.rs"), false);
	assert.match(hookLog, /hadolint Dockerfile/);
	assert.match(result.stderr, /Ruff reported unresolved issues:/);
	assert.match(result.stderr, /Rust linter reported unresolved issues:/);
	assert.match(result.stderr, /Hadolint reported unresolved issues:/);
	assert.match(result.stderr, /ruff unresolved in app\.py/);
	assert.match(result.stderr, /clippy unresolved/);
	assert.match(result.stderr, /hadolint unresolved in Dockerfile/);
});

test("main hook uses the first matching pipeline", (t) => {
	const repo = setupRepo(t, "hook-pipeline-order-");
	seedRepo(repo);
	fs.writeFileSync(
		path.join(repo, ".github", "hooks", "postToolUse", "linters.json"),
		JSON.stringify(
			{
				tools: [
					{ id: "first-tool-check", command: "first-tool", args: ["check"] },
					{ id: "second-tool-check", command: "second-tool", args: ["check"] },
				],
				pipelines: [
					{
						id: "all",
						matcher: ["\\.md$", "\\.ts$"],
						steps: [{ tools: ["first-tool-check"] }],
					},
					{
						id: "scripts",
						matcher: ["\\.ts$"],
						steps: [{ tools: ["second-tool-check"] }],
					},
				],
			},
			null,
			2,
		),
		"utf8",
	);
	fs.writeFileSync(
		path.join(repo, "index.ts"),
		"export const value=1\n",
		"utf8",
	);

	const { env, logFile } = createToolStubs(repo, {
		markdownlint: false,
		biome: false,
		oxlint: false,
		eslint: false,
		firstTool: true,
		secondTool: true,
	});
	const result = runHook(repo, env);
	const hookLog = fs.readFileSync(logFile, "utf8");

	assert.equal(result.status, 0);
	assert.match(hookLog, /first-tool check index\.ts/);
	assert.equal(hookLog.includes("second-tool check index.ts"), false);
});

test("main hook skips all work when tool use is denied", (t) => {
	const repo = setupRepo(t, "hook-denied-");
	seedRepo(repo);
	makeFilesDirty(repo);

	const { env, logFile } = createToolStubs(repo);
	const result = runHook(repo, env, "denied");

	assert.equal(result.status, 0);
	assert.equal(fs.existsSync(logFile), false);
	assert.equal(result.stdout, "");
	assert.equal(result.stderr, "");
});
