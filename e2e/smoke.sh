#!/usr/bin/env bash
# 끝에서 끝 스모크는 Windows 호스트(Jenkins 서비스 에이전트)에서 돈다 — e2e/smoke.ps1 (Jenkinsfile.windows 의 Smoke 단계). 리눅스에서는 하지 않는다.
echo "끝에서 끝 스모크는 Windows 전용이다: e2e/smoke.ps1 (docs/windows-host.md)" >&2
exit 2
