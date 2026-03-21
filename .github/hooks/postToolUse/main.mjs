#!/usr/bin/env node

import fs from "node:fs";
import process from "node:process";
import {
	getChangedFiles,
	getRepoRoot,
	listDirtyFiles,
	loadState,
	readStdin,
	resolveStateFilePath,
	runCommand,
	saveState,
	toRelativeRepoPath,
	writeResultOutput,
} from "./lib/incremental-files.mjs";

const STATE_SUBPATH = [".copilot-hooks", "post-tool-use-state.json"];
const config = loadLintersConfig();

function loadLintersConfig() {
	const raw = fs.readFileSync(
		new URL("./linters.json", import.meta.url),
		"utf8",
	);
	const parsed = JSON.parse(raw);
	const tools = new Map();
	const pipelines = [];

	for (const tool of parsed.tools ?? []) {
		if (tools.has(tool.id)) {
			throw new Error(`Duplicate tool id: ${tool.id}`);
		}

		if (tool.args !== undefined && !Array.isArray(tool.args)) {
			throw new Error(`Tool "${tool.id}" args must be an array.`);
		}

		if (
			tool.appendFiles !== undefined &&
			typeof tool.appendFiles !== "boolean"
		) {
			throw new Error(`Tool "${tool.id}" appendFiles must be a boolean.`);
		}

		tools.set(tool.id, {
			...tool,
			args: tool.args ?? [],
			appendFiles: tool.appendFiles ?? true,
		});
	}

	for (const pipeline of parsed.pipelines ?? []) {
		if (pipelines.some((entry) => entry.id === pipeline.id)) {
			throw new Error(`Duplicate pipeline id: ${pipeline.id}`);
		}

		if (!Array.isArray(pipeline.steps) || pipeline.steps.length === 0) {
			throw new Error(
				`Pipeline "${pipeline.id}" must define at least one step.`,
			);
		}

		for (const step of pipeline.steps) {
			if (!Array.isArray(step.tools) || step.tools.length === 0) {
				throw new Error(
					`Each step in pipeline "${pipeline.id}" must define at least one tool.`,
				);
			}

			for (const toolId of step.tools) {
				if (!tools.has(toolId)) {
					throw new Error(
						`Unknown tool referenced by pipeline "${pipeline.id}": ${toolId}`,
					);
				}
			}
		}

		pipelines.push({
			...pipeline,
			matcher: compileMatcher(pipeline.id, pipeline.matcher),
		});
	}

	return {
		tools,
		pipelines,
	};
}

function compileMatcher(pipelineId, matcher) {
	if (!Array.isArray(matcher) || matcher.length === 0) {
		throw new Error(
			`Pipeline "${pipelineId}" must define matcher as a non-empty array.`,
		);
	}

	if (
		matcher.some((pattern) => typeof pattern !== "string" || pattern === "")
	) {
		throw new Error(
			`Pipeline "${pipelineId}" matcher entries must be non-empty regex strings.`,
		);
	}

	return new RegExp(matcher.map((pattern) => `(?:${pattern})`).join("|"));
}

function matchesPipeline(pipeline, relativePath) {
	return pipeline.matcher.test(relativePath);
}

function classifyFilesByPipeline(repoRoot, files) {
	const filesByPipeline = new Map(
		config.pipelines.map((pipeline) => [pipeline.id, []]),
	);
	const matchedFiles = [];

	for (const filePath of files) {
		const relativePath = toRelativeRepoPath(repoRoot, filePath);
		for (const pipeline of config.pipelines) {
			if (!matchesPipeline(pipeline, relativePath)) {
				continue;
			}

			filesByPipeline.get(pipeline.id).push(relativePath);
			matchedFiles.push(filePath);
			break;
		}
	}

	return { matchedFiles, filesByPipeline };
}

function getCurrentRelevantFiles(repoRoot) {
	return classifyFilesByPipeline(repoRoot, listDirtyFiles(repoRoot));
}

function runStepWithFallback(repoRoot, toolIds, files) {
	const attempted = [];

	for (const toolId of toolIds) {
		const tool = config.tools.get(toolId);
		const args = tool.appendFiles ? [...tool.args, ...files] : tool.args;
		const result = runCommand(tool.command, args, repoRoot);

		if (!result.error) {
			return result;
		}

		if (result.error.code !== "ENOENT") {
			throw result.error;
		}

		attempted.push(`${toolId} (${tool.command})`);
	}

	return {
		status: 1,
		stdout: "",
		stderr: `No available tool found. Tried: ${attempted.join(", ")}\n`,
	};
}

function runPipelines(repoRoot, filesByPipeline) {
	let exitCode = 0;
	let hasReportedFailure = false;

	for (const pipeline of config.pipelines) {
		const files = filesByPipeline.get(pipeline.id) ?? [];
		if (files.length === 0) {
			continue;
		}

		for (const step of pipeline.steps) {
			const result = runStepWithFallback(repoRoot, step.tools, files);

			if ((result.status ?? 0) === 0 || !step.reportFailure) {
				continue;
			}

			if (step.failureLabel) {
				if (hasReportedFailure) {
					process.stderr.write("\n");
				}
				process.stderr.write(`${step.failureLabel}\n`);
			}

			writeResultOutput(result);
			exitCode = Math.max(exitCode, result.status ?? 1);
			hasReportedFailure = true;
		}
	}

	return exitCode;
}

async function main() {
	const rawInput = await readStdin();
	if (rawInput.trim() === "") {
		return;
	}

	const input = JSON.parse(rawInput);
	if (input.toolResult?.resultType === "denied") {
		return;
	}

	const cwd =
		typeof input.cwd === "string" && input.cwd !== ""
			? input.cwd
			: process.cwd();
	const repoRoot = getRepoRoot(cwd);
	const stateFilePath = resolveStateFilePath(repoRoot, STATE_SUBPATH);
	const previousSignatures = loadState(stateFilePath);
	const currentRelevantFiles = getCurrentRelevantFiles(repoRoot);
	const changedFiles = getChangedFiles(
		repoRoot,
		currentRelevantFiles.matchedFiles,
		previousSignatures,
	);

	if (changedFiles.length === 0) {
		saveState(stateFilePath, repoRoot, currentRelevantFiles.matchedFiles);
		return;
	}

	process.exitCode = runPipelines(
		repoRoot,
		classifyFilesByPipeline(repoRoot, changedFiles).filesByPipeline,
	);
	saveState(
		stateFilePath,
		repoRoot,
		getCurrentRelevantFiles(repoRoot).matchedFiles,
	);
}

main().catch((error) => {
	const message =
		error instanceof Error ? (error.stack ?? error.message) : String(error);
	console.error(message);
	process.exitCode = 1;
});
