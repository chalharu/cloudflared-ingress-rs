import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

export function readStdin() {
	return new Promise((resolve, reject) => {
		let data = "";
		process.stdin.setEncoding("utf8");
		process.stdin.on("data", (chunk) => {
			data += chunk;
		});
		process.stdin.on("end", () => resolve(data));
		process.stdin.on("error", reject);
	});
}

export function unique(values) {
	return [...new Set(values)];
}

export function runCommand(command, args, cwd, options = {}) {
	return spawnSync(command, args, {
		cwd,
		encoding: "utf8",
		stdio: "pipe",
		...options,
	});
}

export function splitNulSeparated(output) {
	if (!output) {
		return [];
	}

	return output
		.split("\0")
		.map((item) => item.trim())
		.filter((item) => item !== "");
}

export function getRepoRoot(cwd) {
	const result = runCommand("git", ["rev-parse", "--show-toplevel"], cwd);
	if (result.status !== 0 || result.error) {
		return cwd;
	}

	const repoRoot = result.stdout.trim();
	return repoRoot === "" ? cwd : repoRoot;
}

export function getGitDir(repoRoot) {
	const result = runCommand("git", ["rev-parse", "--git-dir"], repoRoot);
	if (result.status !== 0 || result.error) {
		return path.join(repoRoot, ".git");
	}

	const gitDir = result.stdout.trim();
	return path.resolve(repoRoot, gitDir);
}

export function listGitPaths(repoRoot, args) {
	const result = runCommand("git", args, repoRoot);
	if (result.error) {
		throw result.error;
	}

	if (result.status !== 0) {
		return [];
	}

	return splitNulSeparated(result.stdout);
}

export function toRelativeRepoPath(repoRoot, filePath) {
	return path.relative(repoRoot, filePath).split(path.sep).join("/");
}

export function getFileSignature(filePath) {
	const stat = fs.statSync(filePath);
	return `${stat.size}:${stat.mtimeMs}`;
}

export function loadState(stateFilePath) {
	if (!fs.existsSync(stateFilePath)) {
		return {};
	}

	const raw = fs.readFileSync(stateFilePath, "utf8");
	const parsed = JSON.parse(raw);
	if (
		!parsed ||
		typeof parsed !== "object" ||
		!parsed.signatures ||
		typeof parsed.signatures !== "object"
	) {
		return {};
	}

	return parsed.signatures;
}

export function saveState(stateFilePath, repoRoot, files) {
	fs.mkdirSync(path.dirname(stateFilePath), { recursive: true });
	const payload = {
		version: 1,
		repoRoot,
		signatures: Object.fromEntries(
			files.map((filePath) => [
				toRelativeRepoPath(repoRoot, filePath),
				getFileSignature(filePath),
			]),
		),
	};
	fs.writeFileSync(
		stateFilePath,
		`${JSON.stringify(payload, null, 2)}\n`,
		"utf8",
	);
}

export function resolveStateFilePath(repoRoot, stateSubpath) {
	return path.join(getGitDir(repoRoot), ...stateSubpath);
}

export function getChangedFiles(repoRoot, currentFiles, previousSignatures) {
	return currentFiles.filter((filePath) => {
		const relativePath = toRelativeRepoPath(repoRoot, filePath);
		return previousSignatures[relativePath] !== getFileSignature(filePath);
	});
}

export function listDirtyFiles(repoRoot) {
	const candidates = unique([
		...listGitPaths(repoRoot, [
			"diff",
			"--name-only",
			"-z",
			"--diff-filter=ACMRTUXB",
		]),
		...listGitPaths(repoRoot, [
			"diff",
			"--cached",
			"--name-only",
			"-z",
			"--diff-filter=ACMRTUXB",
		]),
		...listGitPaths(repoRoot, [
			"ls-files",
			"--others",
			"--exclude-standard",
			"-z",
		]),
	]);

	return candidates
		.map((relativePath) => path.resolve(repoRoot, relativePath))
		.filter(
			(filePath) => fs.existsSync(filePath) && fs.statSync(filePath).isFile(),
		);
}

export function writeResultOutput(result) {
	if (result.stdout) {
		process.stdout.write(result.stdout);
	}

	if (result.stderr) {
		process.stderr.write(result.stderr);
	}
}
