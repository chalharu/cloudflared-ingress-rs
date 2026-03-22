import assert from "node:assert/strict";
import test from "node:test";

import {
	bumpVersion,
	readCurrentVersion,
	selectSemverBump,
	updateCargoLockVersion,
	updateCargoVersion,
	updateChartVersions,
} from "./semver-release.mjs";

const cargoToml = `[package]
name = "cloudflared-ingress-rs"
version = "0.1.2"
edition = "2024"

[dependencies]
serde = "1.0"
`;

const cargoLock = `[[package]]
name = "cloudflared-ingress-rs"
version = "0.1.2"
dependencies = [
 "serde",
]
`;

const chartYaml = `apiVersion: v2
name: cloudflared-ingress
version: 0.1.2
appVersion: "0.1.2"
`;

test("selectSemverBump requires exactly one release label unless a default is supplied", () => {
	assert.equal(selectSemverBump(["docs", "semver:patch"]), "patch");
	assert.equal(selectSemverBump(["docs"], "patch"), "patch");
	assert.throws(
		() => selectSemverBump(["semver:major", "semver:minor"]),
		/expected exactly one semver label/,
	);
	assert.throws(
		() => selectSemverBump(["docs"]),
		/expected exactly one semver label/,
	);
	assert.throws(
		() => selectSemverBump(["docs"], "bogus"),
		/unsupported default semver bump kind/,
	);
});

test("bumpVersion increments semantic versions by the requested level", () => {
	assert.equal(bumpVersion("0.1.2", "patch"), "0.1.3");
	assert.equal(bumpVersion("0.1.2", "minor"), "0.2.0");
	assert.equal(bumpVersion("0.1.2", "major"), "1.0.0");
});

test("readCurrentVersion rejects mismatched version sources", () => {
	assert.equal(readCurrentVersion(cargoToml, chartYaml, cargoLock), "0.1.2");
	assert.throws(
		() =>
			readCurrentVersion(
				cargoToml,
				chartYaml.replace('appVersion: "0.1.2"', 'appVersion: "0.1.3"'),
				cargoLock,
			),
		/version files disagree/,
	);
});

test("readCurrentVersion accepts an explicit current release version override", () => {
	assert.equal(
		readCurrentVersion(
			cargoToml,
			chartYaml.replace('appVersion: "0.1.2"', 'appVersion: "9.9.9"'),
			cargoLock,
			"0.1.2",
		),
		"0.1.2",
	);
	assert.throws(
		() => readCurrentVersion(cargoToml, chartYaml, cargoLock, "not-semver"),
		/invalid semantic version/,
	);
});

test("update helpers rewrite the project version in every managed file", () => {
	const nextVersion = "0.1.3";

	assert.match(updateCargoVersion(cargoToml, nextVersion), /version = "0.1.3"/);
	assert.match(
		updateCargoLockVersion(cargoLock, nextVersion),
		/version = "0.1.3"/,
	);
	assert.match(
		updateChartVersions(chartYaml, nextVersion),
		/^version: 0.1.3$/m,
	);
	assert.match(
		updateChartVersions(chartYaml, nextVersion),
		/^appVersion: "0.1.3"$/m,
	);
});
