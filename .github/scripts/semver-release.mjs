import { appendFile, readFile, writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

export const SEMVER_LABELS = ["semver:major", "semver:minor", "semver:patch"];
export const NON_RELEASE_FILE_PATTERNS = [
	/^\.github\//,
	/(^|\/)[^/]+\.md$/,
	/^LICENSE$/,
	/^renovate\.json$/,
	/^sonar-project\.properties$/,
];

const SUPPORTED_SEMVER_BUMPS = ["major", "minor", "patch"];

const semverPattern =
	/^(?<major>0|[1-9]\d*)\.(?<minor>0|[1-9]\d*)\.(?<patch>0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

function readJsonEnv(name, fallback = "[]") {
	const value = process.env[name] ?? fallback;

	try {
		return JSON.parse(value);
	} catch (error) {
		throw new Error(`invalid JSON in ${name}: ${error.message}`);
	}
}

function ensureStringArray(value, name) {
	if (
		!Array.isArray(value) ||
		value.some((entry) => typeof entry !== "string")
	) {
		throw new Error(`${name} must be a JSON array of strings`);
	}

	return value;
}

function ensureSupportedSemverBump(bump) {
	if (!SUPPORTED_SEMVER_BUMPS.includes(bump)) {
		throw new Error(`unsupported default semver bump kind: ${bump}`);
	}

	return bump;
}

export function parseSemver(version) {
	const match = semverPattern.exec(version);
	if (!match?.groups) {
		throw new Error(`invalid semantic version: ${version}`);
	}

	return {
		major: Number.parseInt(match.groups.major, 10),
		minor: Number.parseInt(match.groups.minor, 10),
		patch: Number.parseInt(match.groups.patch, 10),
	};
}

export function selectSemverBump(labels, defaultBump) {
	const matched = labels.filter((label) => SEMVER_LABELS.includes(label));

	if (matched.length === 0) {
		if (defaultBump) {
			return ensureSupportedSemverBump(defaultBump);
		}

		throw new Error(
			`expected exactly one semver label (${SEMVER_LABELS.join(", ")}), found: none`,
		);
	}

	if (matched.length !== 1) {
		throw new Error(
			`expected exactly one semver label (${SEMVER_LABELS.join(", ")}), found: ${matched.join(", ")}`,
		);
	}

	return matched[0].slice("semver:".length);
}

export function hasOnlyNonReleaseChanges(changedFiles) {
	const files = ensureStringArray(changedFiles, "changedFiles");

	return (
		files.length > 0 &&
		files.every((path) =>
			NON_RELEASE_FILE_PATTERNS.some((pattern) => pattern.test(path)),
		)
	);
}

export function selectReleaseStrategy(labels, changedFiles, defaultBump) {
	const matched = labels.filter((label) => SEMVER_LABELS.includes(label));
	const files = ensureStringArray(changedFiles, "changedFiles");

	if (matched.length === 0 && files.length === 0) {
		return {
			shouldRelease: false,
			reason: "no-files-changed",
		};
	}

	if (matched.length === 0 && hasOnlyNonReleaseChanges(files)) {
		return {
			shouldRelease: false,
			reason: "non-release-files-only",
		};
	}

	const bump = selectSemverBump(labels, defaultBump);

	return {
		shouldRelease: true,
		bump,
		reason: matched.length === 1 ? "semver-label" : "default-bump",
	};
}

export function bumpVersion(currentVersion, bump) {
	const { major, minor, patch } = parseSemver(currentVersion);

	switch (bump) {
		case "major":
			return `${major + 1}.0.0`;
		case "minor":
			return `${major}.${minor + 1}.0`;
		case "patch":
			return `${major}.${minor}.${patch + 1}`;
		default:
			throw new Error(`unsupported semver bump kind: ${bump}`);
	}
}

export function extractCargoVersion(text) {
	const match = /^(\[package\][\s\S]*?^version = ")([^"]+)(")/m.exec(text);
	if (!match) {
		throw new Error("failed to locate [package] version in Cargo.toml");
	}

	return match[2];
}

export function updateCargoVersion(text, newVersion) {
	const updated = text.replace(
		/^(\[package\][\s\S]*?^version = ")([^"]+)(")/m,
		`$1${newVersion}$3`,
	);

	if (updated === text) {
		throw new Error("failed to update Cargo.toml version");
	}

	return updated;
}

export function extractCargoLockVersion(text) {
	const match =
		/(\[\[package\]\]\nname = "cloudflared-ingress-rs"\nversion = ")([^"]+)(")/.exec(
			text,
		);
	if (!match) {
		throw new Error(
			"failed to locate cloudflared-ingress-rs version in Cargo.lock",
		);
	}

	return match[2];
}

export function updateCargoLockVersion(text, newVersion) {
	const updated = text.replace(
		/(\[\[package\]\]\nname = "cloudflared-ingress-rs"\nversion = ")([^"]+)(")/,
		`$1${newVersion}$3`,
	);

	if (updated === text) {
		throw new Error("failed to update Cargo.lock root package version");
	}

	return updated;
}

export function extractChartVersions(text) {
	const chartVersionMatch = /^version:\s*"?([^"\n]+)"?\s*$/m.exec(text);
	const appVersionMatch = /^appVersion:\s*"?([^"\n]+)"?\s*$/m.exec(text);

	if (!chartVersionMatch || !appVersionMatch) {
		throw new Error("failed to locate version fields in helm/Chart.yaml");
	}

	return {
		chartVersion: chartVersionMatch[1].trim(),
		appVersion: appVersionMatch[1].trim(),
	};
}

export function updateChartVersions(text, newVersion) {
	const withChartVersion = text.replace(
		/^version:\s*"?([^"\n]+)"?\s*$/m,
		`version: ${newVersion}`,
	);
	const updated = withChartVersion.replace(
		/^appVersion:\s*"?([^"\n]+)"?\s*$/m,
		`appVersion: "${newVersion}"`,
	);

	if (updated === text) {
		throw new Error("failed to update helm/Chart.yaml versions");
	}

	return updated;
}

export function readCurrentVersion(
	cargoTomlText,
	chartText,
	cargoLockText,
	currentVersionOverride,
) {
	const { chartVersion, appVersion } = extractChartVersions(chartText);
	const versions = [
		extractCargoVersion(cargoTomlText),
		chartVersion,
		appVersion,
	];

	if (cargoLockText !== undefined && cargoLockText !== null) {
		versions.push(extractCargoLockVersion(cargoLockText));
	}

	if (currentVersionOverride) {
		parseSemver(currentVersionOverride);
		return currentVersionOverride;
	}

	const distinctVersions = [...new Set(versions)];
	if (distinctVersions.length !== 1) {
		throw new Error(`version files disagree: ${distinctVersions.join(", ")}`);
	}

	parseSemver(distinctVersions[0]);
	return distinctVersions[0];
}

async function writeOutputs(entries) {
	if (!process.env.GITHUB_OUTPUT) {
		return;
	}

	const lines = Object.entries(entries).map(
		([key, value]) => `${key}=${value}`,
	);
	await appendFile(process.env.GITHUB_OUTPUT, `${lines.join("\n")}\n`);
}

export async function validateLabelsFromEnv() {
	const labels = ensureStringArray(
		readJsonEnv("PR_LABELS_JSON"),
		"PR_LABELS_JSON",
	);
	const bump = selectSemverBump(labels);

	console.log(`validated semver label: ${bump}`);
	await writeOutputs({ bump });
}

export async function evaluateReleaseFromEnv() {
	const labels = ensureStringArray(
		readJsonEnv("PR_LABELS_JSON"),
		"PR_LABELS_JSON",
	);
	const changedFiles = ensureStringArray(
		readJsonEnv("CHANGED_FILES_JSON"),
		"CHANGED_FILES_JSON",
	);
	const defaultBump = process.env.DEFAULT_SEMVER_BUMP;
	const decision = selectReleaseStrategy(labels, changedFiles, defaultBump);

	if (decision.shouldRelease) {
		console.log(`release required via ${decision.reason}: ${decision.bump}`);
	} else {
		console.log("release skipped: merged changes only touch non-release files");
	}

	await writeOutputs({
		should_release: decision.shouldRelease,
		bump: decision.bump ?? "",
		reason: decision.reason,
	});
}

export async function bumpProjectVersionFromEnv() {
	const labels = ensureStringArray(
		readJsonEnv("PR_LABELS_JSON"),
		"PR_LABELS_JSON",
	);
	const defaultBump = process.env.DEFAULT_SEMVER_BUMP;
	const bump = selectSemverBump(labels, defaultBump);
	const cargoTomlPath = process.env.CARGO_TOML_PATH ?? "Cargo.toml";
	const cargoLockPath = process.env.CARGO_LOCK_PATH ?? "Cargo.lock";
	const chartPath = process.env.CHART_PATH ?? "helm/Chart.yaml";
	const currentVersionOverride = process.env.CURRENT_VERSION;
	const [cargoTomlText, chartText, cargoLockText] = await Promise.all([
		readFile(cargoTomlPath, "utf8"),
		readFile(chartPath, "utf8"),
		readFile(cargoLockPath, "utf8"),
	]);
	const currentVersion = readCurrentVersion(
		cargoTomlText,
		chartText,
		cargoLockText,
		currentVersionOverride,
	);
	const nextVersion = bumpVersion(currentVersion, bump);

	await Promise.all([
		writeFile(cargoTomlPath, updateCargoVersion(cargoTomlText, nextVersion)),
		writeFile(
			cargoLockPath,
			updateCargoLockVersion(cargoLockText, nextVersion),
		),
		writeFile(chartPath, updateChartVersions(chartText, nextVersion)),
	]);

	console.log(`bumped version ${currentVersion} -> ${nextVersion}`);
	await writeOutputs({
		bump,
		previous_version: currentVersion,
		version: nextVersion,
	});
}

async function main() {
	const command = process.argv[2];

	switch (command) {
		case "validate-labels":
			await validateLabelsFromEnv();
			break;
		case "evaluate-release":
			await evaluateReleaseFromEnv();
			break;
		case "bump":
			await bumpProjectVersionFromEnv();
			break;
		default:
			throw new Error(
				"usage: semver-release.mjs <validate-labels|evaluate-release|bump>",
			);
	}
}

if (
	process.argv[1] &&
	import.meta.url === pathToFileURL(process.argv[1]).href
) {
	main().catch((error) => {
		console.error(error.message);
		process.exitCode = 1;
	});
}
