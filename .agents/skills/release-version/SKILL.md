---
name: release-version
description: Create and push a semver-compatible timestamped release tag for this repository. Use when the user asks to publish a version, run "/release-version" or "release-version", says "发布版本", "发版", "打 tag", or wants a China Standard Time semver release tag, with all release-owned version files synchronized to the tag (including `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, and the `codeg` package entry in `src-tauri/Cargo.lock`) and committed when needed, then pushed to origin with the current branch.
---

# Release Version

## Purpose

Publish the current repository state by creating a China Standard Time semver-compatible release tag, ensuring all release-owned version files match it first, then pushing the current branch and the new tag to `origin`.

Tag format is always:

```bash
vYY.M.DHHMM
```

Example: `v26.6.292023`.

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
tag="$(TZ=Asia/Shanghai node -e '
  const now = new Date();
  const yy = String(now.getFullYear()).slice(-2);
  const month = now.getMonth() + 1;
  const day = now.getDate();
  const hh = String(now.getHours()).padStart(2, "0");
  const mm = String(now.getMinutes()).padStart(2, "0");
  process.stdout.write(`v${yy}.${month}.${day}${hh}${mm}`);
')"
```

Validate the tag shape:

```bash
TAG="$tag" node -e '
  const tag = process.env.TAG;
  const match = /^v(\d{2})\.(\d+)\.(\d+)$/.exec(tag);
  if (!match) {
    throw new Error(`Invalid release tag: ${tag}`);
  }

  const month = Number(match[2]);
  const patch = Number(match[3]);
  const day = Math.floor(patch / 10000);
  const hhmm = patch % 10000;
  const hour = Math.floor(hhmm / 100);
  const minute = hhmm % 100;

  if (
    !Number.isInteger(month) ||
    !Number.isInteger(day) ||
    !Number.isInteger(hour) ||
    !Number.isInteger(minute) ||
    month < 1 ||
    month > 12 ||
    day < 1 ||
    day > 31 ||
    hour < 0 ||
    hour > 23 ||
    minute < 0 ||
    minute > 59
  ) {
    throw new Error(`Invalid release tag: ${tag}`);
  }
'
```

Before creating it, verify the tag does not already exist locally or on `origin`:

```bash
git fetch --tags origin
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && echo "Local tag exists: $tag" && exit 1
git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1 && echo "Remote tag exists: $tag" && exit 1
```

If the tag already exists because another release happened in the same minute, wait until the next China Standard Time minute, regenerate the tag, and re-run the checks. Do not reuse or move the existing tag.

### 3. Synchronize all release-owned version files

Before creating the tag, verify that all release-owned version files match the candidate tag, then update only those owned version fields to the new tag version:

```bash
new_version="${tag#v}"
package_version="$(node -p "require('./package.json').version")"
cargo_version="$(node -e '
  const fs = require("fs");
  const text = fs.readFileSync("src-tauri/Cargo.toml", "utf8");
  const match = text.match(/^version = "([^"]+)"/m);
  if (!match) {
    throw new Error("version field not found in src-tauri/Cargo.toml");
  }
  process.stdout.write(match[1]);
')"
tauri_version="$(node -p "require('./src-tauri/tauri.conf.json').version")"
lock_version="$(node -e '
  const fs = require("fs");
  const text = fs.readFileSync("src-tauri/Cargo.lock", "utf8");
  const match = text.match(/\[\[package\]\]\nname = "codeg"\nversion = "([^"]+)"/);
  if (!match) {
    throw new Error("codeg package version not found in src-tauri/Cargo.lock");
  }
  process.stdout.write(match[1]);
')"

if [ "$package_version" != "$new_version" ] || \
   [ "$cargo_version" != "$new_version" ] || \
   [ "$tauri_version" != "$new_version" ] || \
   [ "$lock_version" != "$new_version" ]; then
  NEW_VERSION="$new_version" \
  node -e '
    const fs = require("fs");

    const updates = [
      {
        path: "package.json",
        pattern: /"version"\s*:\s*"[^"]+"/,
        replacement: `"version": "${process.env.NEW_VERSION}"`,
        missing: "version field not found in package.json",
      },
      {
        path: "src-tauri/Cargo.toml",
        pattern: /^version = "[^"]+"/m,
        replacement: `version = "${process.env.NEW_VERSION}"`,
        missing: "version field not found in src-tauri/Cargo.toml",
      },
      {
        path: "src-tauri/tauri.conf.json",
        pattern: /"version"\s*:\s*"[^"]+"/,
        replacement: `"version": "${process.env.NEW_VERSION}"`,
        missing: "version field not found in src-tauri/tauri.conf.json",
      },
      {
        path: "src-tauri/Cargo.lock",
        pattern: /(\[\[package\]\]\nname = "codeg"\nversion = ")[^"]+(")/,
        replacement: `$1${process.env.NEW_VERSION}$2`,
        missing: "codeg package version not found in src-tauri/Cargo.lock",
      },
    ];

    for (const update of updates) {
      const text = fs.readFileSync(update.path, "utf8");
      if (!update.pattern.test(text)) {
        throw new Error(update.missing);
      }

      fs.writeFileSync(
        update.path,
        text.replace(update.pattern, update.replacement),
      );
    }
  '

  package_version="$(node -p "require('./package.json').version")"
  cargo_version="$(node -e '
    const fs = require("fs");
    const text = fs.readFileSync("src-tauri/Cargo.toml", "utf8");
    const match = text.match(/^version = "([^"]+)"/m);
    if (!match) {
      throw new Error("version field not found in src-tauri/Cargo.toml");
    }
    process.stdout.write(match[1]);
  ')"
  tauri_version="$(node -p "require('./src-tauri/tauri.conf.json').version")"
  lock_version="$(node -e '
    const fs = require("fs");
    const text = fs.readFileSync("src-tauri/Cargo.lock", "utf8");
    const match = text.match(/\[\[package\]\]\nname = "codeg"\nversion = "([^"]+)"/);
    if (!match) {
      throw new Error("codeg package version not found in src-tauri/Cargo.lock");
    }
    process.stdout.write(match[1]);
  ')"

  if [ "$package_version" != "$new_version" ] || \
     [ "$cargo_version" != "$new_version" ] || \
     [ "$tauri_version" != "$new_version" ] || \
     [ "$lock_version" != "$new_version" ]; then
    echo "Failed to synchronize all release-owned version files"
    exit 1
  fi

  git diff --check -- package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json src-tauri/Cargo.lock
  git diff -- package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json src-tauri/Cargo.lock
  git add package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json src-tauri/Cargo.lock
  git commit -m "chore(release): bump version to $new_version"
fi
```

Rules:

- The release tag is the source of truth. `package.json.version`, `src-tauri/Cargo.toml.version`, `src-tauri/tauri.conf.json.version`, and the `codeg` package version in `src-tauri/Cargo.lock` must all equal the tag without the leading `v`.
- The tag format itself must already be valid semver after removing the leading `v`.
- Update only release-owned version fields. Do not replace third-party dependency versions that happen to equal the old app version, such as unrelated entries in `Cargo.lock`.
- If the version does not match, commit the synchronized version files before creating the tag.
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
