---
name: release-version
description: Create and push a timestamped release tag for this repository. Use when the user asks to publish a version, run "/release-version" or "release-version", says "发布版本", "发版", "打 tag", or wants a China Standard Time format vYY.MMDD.HHMM release tag, with src-tauri/tauri.conf.json version synchronized and committed when needed, then pushed to origin with the current branch.
---

# Release Version

## Purpose

Publish the current repository state by creating a China Standard Time release tag, ensuring `src-tauri/tauri.conf.json` has a matching version first, then pushing the current branch and the new tag to `origin`.

Tag format is always:

```bash
vYY.MMDD.HHMM
```

Example: `v26.0601.2355`.

## Workflow

### 1. Preflight

Run from the repository root.

Check the repository state before creating any tag:

```bash
git rev-parse --show-toplevel
git status --short
git branch --show-current
git remote get-url origin
git rev-parse --short HEAD
```

Rules:

- If the working tree has uncommitted changes, stop and explain that the release tag can only include committed code. Do not stash, commit, discard, or continue unless the user explicitly says to release the current `HEAD` anyway.
- If the repository is in detached `HEAD`, stop and ask which branch should be pushed.
- If `origin` is missing, stop and ask for the target remote.
- Never use force push, delete tags, move tags, or overwrite existing tags.

### 2. Generate the candidate tag

Generate the tag with China Standard Time explicitly, regardless of the machine's local timezone:

```bash
tag="$(TZ=Asia/Shanghai date '+v%y.%m%d.%H%M')"
```

Validate the tag shape:

```bash
case "$tag" in
  v[0-9][0-9].[0-9][0-9][0-9][0-9].[0-9][0-9][0-9][0-9]) ;;
  *) echo "Invalid release tag: $tag"; exit 1 ;;
esac
```

Before creating it, verify the tag does not already exist locally or on `origin`:

```bash
git fetch --tags origin
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && echo "Local tag exists: $tag" && exit 1
git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1 && echo "Remote tag exists: $tag" && exit 1
```

If the tag already exists because another release happened in the same minute, wait until the next China Standard Time minute, regenerate the tag, and re-run the checks. Do not reuse or move the existing tag.

### 3. Synchronize the app version

Before creating the tag, verify that the candidate tag matches the Tauri app version:

```bash
version="$(node -p "require('./src-tauri/tauri.conf.json').version")"
if [ "v$version" != "$tag" ]; then
  new_version="${tag#v}"
  NEW_VERSION="$new_version" node -e '
    const fs = require("fs");
    const path = "src-tauri/tauri.conf.json";
    const version = process.env.NEW_VERSION;
    const text = fs.readFileSync(path, "utf8");
    if (!/"version"\s*:\s*"[^"]+"/.test(text)) {
      throw new Error("version field not found in src-tauri/tauri.conf.json");
    }
    fs.writeFileSync(
      path,
      text.replace(/"version"\s*:\s*"[^"]+"/, `"version": "${version}"`),
    );
  '

  git diff --check -- src-tauri/tauri.conf.json
  git diff -- src-tauri/tauri.conf.json
  git add src-tauri/tauri.conf.json
  git commit -m "chore(release): bump version to $new_version"
fi
```

Rules:

- The release tag is the source of truth. `src-tauri/tauri.conf.json.version` must equal the tag without the leading `v`.
- If the version does not match, commit only `src-tauri/tauri.conf.json` before creating the tag.
- Use the latest commit after the version bump as the tag target.
- If the version already matches, do not create a version bump commit.
- If the version update introduces unexpected file changes, stop and report the diff instead of committing.

After this step, verify the working tree is clean and refresh the commit hash:

```bash
git status --short
git rev-parse --short HEAD
```

### 4. Create the tag on the latest commit

Use an annotated tag so the release has a stable message:

```bash
git tag -a "$tag" -m "Release $tag" HEAD
```

Confirm the tag points at the intended commit:

```bash
git rev-list -n 1 "$tag"
git rev-parse HEAD
```

The two full commit hashes must match.

### 5. Push code and tag

Push the current branch and only the new tag:

```bash
branch="$(git branch --show-current)"
git push origin "$branch"
git push origin "$tag"
```

Do not use `git push --tags`; it may push unrelated local tags.

### 6. Report

Reply in Chinese with:

- release tag name;
- branch pushed;
- short commit hash;
- whether a version bump commit was created;
- confirmation that both the branch and tag were pushed to `origin`.

If any step fails, report the failed step and the exact recovery needed. Do not silently retry with force options.
