import { pathToFileURL } from "node:url";

const fullSemverTag = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;
const minorSemverTag = /^\d+\.\d+$/;
const majorSemverTag = /^\d+$/;

function requireEnv(name) {
	const value = process.env[name];
	if (!value) {
		throw new Error(`missing required environment variable: ${name}`);
	}

	return value;
}

function packageScope(ownerType) {
	return ownerType === "Organization" ? "orgs" : "users";
}

function packageVersionTimestamp(version) {
	return Date.parse(version.updated_at ?? version.created_at ?? 0);
}

export function packageVersionTags(version) {
	return version.metadata?.container?.tags ?? [];
}

export function isVersionTag(tag) {
	return (
		fullSemverTag.test(tag) ||
		minorSemverTag.test(tag) ||
		majorSemverTag.test(tag)
	);
}

export function selectVersionsToDelete(versions, keepCount) {
	if (!Number.isInteger(keepCount) || keepCount < 0) {
		throw new Error(
			`keepCount must be a non-negative integer, got: ${keepCount}`,
		);
	}

	return versions
		.filter((version) => !packageVersionTags(version).some(isVersionTag))
		.sort(
			(left, right) =>
				packageVersionTimestamp(right) - packageVersionTimestamp(left),
		)
		.slice(keepCount);
}

async function githubRequest(method, path, token) {
	const response = await fetch(`https://api.github.com${path}`, {
		method,
		headers: {
			Accept: "application/vnd.github+json",
			Authorization: `Bearer ${token}`,
			"X-GitHub-Api-Version": "2022-11-28",
		},
	});

	if (!response.ok && response.status !== 204) {
		const body = await response.text();
		throw new Error(`${method} ${path} failed (${response.status}): ${body}`);
	}

	if (response.status === 204) {
		return null;
	}

	return response.json();
}

async function listPackageVersions({ owner, ownerType, packageName, token }) {
	const scope = packageScope(ownerType);
	const versions = [];

	for (let page = 1; ; page += 1) {
		const response = await githubRequest(
			"GET",
			`/${scope}/${encodeURIComponent(owner)}/packages/container/${encodeURIComponent(
				packageName,
			)}/versions?per_page=100&page=${page}`,
			token,
		);
		versions.push(...response);

		if (response.length < 100) {
			return versions;
		}
	}
}

async function deletePackageVersion({
	owner,
	ownerType,
	packageName,
	packageVersionId,
	token,
}) {
	const scope = packageScope(ownerType);
	await githubRequest(
		"DELETE",
		`/${scope}/${encodeURIComponent(owner)}/packages/container/${encodeURIComponent(
			packageName,
		)}/versions/${packageVersionId}`,
		token,
	);
}

async function main() {
	const token = requireEnv("GITHUB_TOKEN");
	const owner =
		process.env.GITHUB_REPOSITORY_OWNER ??
		requireEnv("GITHUB_REPOSITORY").split("/")[0];
	const repositoryName =
		process.env.GITHUB_REPOSITORY?.split("/")[1] ??
		requireEnv("GITHUB_REPOSITORY").split("/")[1];
	const ownerType = process.env.GITHUB_OWNER_TYPE ?? "User";
	const packageName = process.env.GHCR_PACKAGE_NAME ?? repositoryName;
	const keepCount = Number.parseInt(
		process.env.NON_SEMVER_RETENTION_COUNT ?? "10",
		10,
	);
	const versions = await listPackageVersions({
		owner,
		ownerType,
		packageName,
		token,
	});
	const toDelete = selectVersionsToDelete(versions, keepCount);

	console.log(
		`retaining the newest ${keepCount} non-semver or untagged package versions; deleting ${toDelete.length}`,
	);

	for (const version of toDelete) {
		const tags = packageVersionTags(version);
		console.log(
			`deleting package version ${version.id}: ${tags.length > 0 ? tags.join(", ") : "untagged"}`,
		);
		await deletePackageVersion({
			owner,
			ownerType,
			packageName,
			packageVersionId: version.id,
			token,
		});
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
