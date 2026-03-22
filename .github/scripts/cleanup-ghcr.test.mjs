import assert from "node:assert/strict";
import test from "node:test";

import { isVersionTag, selectVersionsToDelete } from "./cleanup-ghcr.mjs";

function packageVersion(id, updatedAt, tags) {
	return {
		id,
		updated_at: updatedAt,
		metadata: {
			container: {
				tags,
			},
		},
	};
}

test("isVersionTag preserves semantic Docker tags", () => {
	assert.equal(isVersionTag("1.2.3"), true);
	assert.equal(isVersionTag("1.2"), true);
	assert.equal(isVersionTag("1"), true);
	assert.equal(isVersionTag("1.2.3-rc.1"), true);
	assert.equal(isVersionTag("latest"), false);
	assert.equal(isVersionTag("sha-abc123"), false);
});

test("selectVersionsToDelete keeps the newest non-semver builds and all semver tags", () => {
	const versions = [
		packageVersion("semver", "2026-03-20T00:00:00Z", ["1.2.3", "latest"]),
		packageVersion("keep-newest", "2026-03-19T00:00:00Z", ["sha-new"]),
		packageVersion("delete-middle", "2026-03-18T00:00:00Z", ["sha-middle"]),
		packageVersion("delete-oldest", "2026-03-17T00:00:00Z", []),
	];

	assert.deepEqual(
		selectVersionsToDelete(versions, 1).map((version) => version.id),
		["delete-middle", "delete-oldest"],
	);
});
