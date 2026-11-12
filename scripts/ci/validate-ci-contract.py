#!/usr/bin/env python3
"""Guard that PromptLab CI/build jobs live in GitHub Actions and Jenkins deploys only."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
errors = []

for path in ("Jenkinsfile.pr", "Jenkinsfile.runners", "Jenkinsfile.windows"):
    if (ROOT / path).exists():
        errors.append(f"obsolete Jenkins build pipeline remains: {path}")

jenkins = (ROOT / "Jenkinsfile").read_text(encoding="utf-8")
active = "\n".join(line.split("//", 1)[0] for line in jenkins.splitlines())
for required in ("deployFrom: 'production'", "images:", "repo:", "envVar:", "composeProject:"):
    if required not in active:
        errors.append(f"Jenkinsfile missing deployment field: {required}")
retired = ("buildSsh", "builder", "buildScript", "buildCredentials", "check:",
           "dockerfile:", "context:", "platform:", "sourceUrl:", "skipCheckedTree", "macWaitSeconds")
for field in retired:
    if field in active:
        errors.append(f"Jenkinsfile still passes retired build/check field: {field}")

workflow_dir = ROOT / ".github/workflows"
for name in ("pr-check.yml", "release.yml", "image.yml", "windows-build.yml", "build-runners.yml"):
    if not (workflow_dir / name).is_file():
        errors.append(f"missing Actions workflow: {name}")

release = (workflow_dir / "release.yml").read_text(encoding="utf-8")
for fragment in ("windows-build.yml", "needs: [check, image, windows]", "production"):
    if fragment not in release:
        errors.append(f"release workflow must gate promotion on: {fragment}")

images = (workflow_dir / "image.yml").read_text(encoding="utf-8")
for image_name in ("sandcorp-promptlab", "sandcorp-promptlab-judge", "sandcorp-promptlab-docker-proxy"):
    if image_name not in images:
        errors.append(f"Actions image workflow missing runtime image: {image_name}")
if "packages: write" not in images or "push: true" not in images or "${{ github.sha }}" not in images:
    errors.append("runtime image workflow must push SHA-tagged GHCR images")

windows = (workflow_dir / "windows-build.yml").read_text(encoding="utf-8")
for fragment in ("pull_request:", "workflow_call:", "runs-on: windows-2022",
                 "scripts\\windows\\build.ps1", "scripts\\windows\\build-installer.ps1"):
    if fragment not in windows:
        errors.append(f"Windows workflow missing: {fragment}")

runners = (workflow_dir / "build-runners.yml").read_text(encoding="utf-8")
for fragment in ("workflow_dispatch:", "scripts/build-runners.sh", "packages: write"):
    if fragment not in runners:
        errors.append(f"runner image workflow missing: {fragment}")

for path in workflow_dir.glob("*.yml"):
    body = path.read_text(encoding="utf-8")
    active_body = "\n".join(line for line in body.splitlines() if not line.lstrip().startswith("#"))
    if "self-hosted" in active_body or "pull_request_target" in active_body:
        errors.append(f"Actions workflow violates hosted-runner/public-PR policy: {path.name}")

if errors:
    print("CI contract validation failed:", *[f"- {error}" for error in errors], sep="\n", file=sys.stderr)
    raise SystemExit(1)
print("CI ownership contract passed")
