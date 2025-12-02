# ⛔ STOP: Git Workflow Check Required ⛔

**Before ANY git operations, you MUST ask the user:**

```bash
# ❌ NEVER DO THIS:
git checkout -b some-branch-name

# ✅ ALWAYS DO THIS FIRST:
# Use ask_followup_question tool with:
# "Which git branch should I work from for this task?"
# Options: ["main", "safe-working-baseline", "issue-235-cloudflare-backend", "Other (please specify)"]
```

**Why this matters:**
- zap-stream-core has multiple active branches
- User knows the correct branching strategy
- Creating wrong branch creates merge conflicts
- The "Github branches" section below lists known branches but user decides which to use

**Rule:** No `git checkout -b`, `git branch`, or `git push` without explicit user approval of the branch name.

---

# ⛔ MANDATORY PRE-COMMIT CHECKLIST ⛔

**🚨 STOP! Before ANY commit, you MUST complete ALL these steps IN ORDER:**

1. ✅ **Run Rust unit tests first - ALL MUST PASS:**
   ```bash
   cd /Users/visitor/Projects/shosho/zap-stream-core
   cargo test
   ```
   **Required outcome:**
   - ✅ ALL tests MUST pass (no failures)
   - ✅ NO new compilation warnings allowed (see "How to Check Warnings" section below)
   - ❌ If ANY test fails or new warnings appear: FIX BEFORE PROCEEDING

2. ✅ **Start Docker integration test (WITH `-d` FLAG!):**
   ```bash
   cd /Users/visitor/Projects/shosho/zap-stream-core/docs/deploy
   docker-compose up --build -d
   ```
   **⚠️ CRITICAL: The `-d` flag is MANDATORY! Without it, you will crash your context window!**

3. ✅ **Use ask_followup_question to wait for user confirmation:**
   ```
   Question: "Docker build started in background. Please check with 'docker ps' or Docker Desktop to verify containers are running, then confirm build completion."
   Options: ["Build completed successfully", "Build failed - show me error"]
   ```

4. ✅ **Test streaming START with ffmpeg - START TEST MUST PASS:**
   ```bash
   ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
     -f lavfi -i sine=frequency=1000:sample_rate=44100 \
     -c:v libx264 -preset veryfast -tune zerolatency \
     -c:a aac -ar 44100 \
     -f flv rtmp://localhost:1935/Basic/81b97dd0-b959-11f0-b22c-d690ca11bae8 \
     </dev/null >/dev/null 2>&1 &
   
   # Wait for stream to start
   sleep 10
   
   # Check logs for START success
   docker logs --tail 50 zap-stream-core-core-1
   ```
   
   **Required logs (MUST see ALL):**
   - ✅ "Published stream request: Basic/81b97dd0-b959-11f0-b22c-d690ca11bae8 [Live]"
   - ✅ "Pipeline run starting"
   - ✅ "Published stream event"
   - ✅ "Created fMP4 initialization segment"
   - ❌ If ANY missing: FIX BEFORE PROCEEDING

5. ✅ **Test streaming END - DO NOT SKIP THIS – END TEST MUST PASS:**
   ```bash
   # Stop the stream
   pkill -9 -f "ffmpeg.*testsrc"
   
   # ⛔ MANDATORY: Wait for shutdown to complete
   sleep 5
   
   # ⛔ MANDATORY: Check logs for END success
   docker logs --tail 50 zap-stream-core-core-1
   ```
   
   **Required logs (MUST see ALL):**
   - ✅ "read_data EOF"
   - ✅ "WARN: Demuxer get_packet failed, entering idle mode"
   - ✅ "Stream ended [stream_id]"
   - ✅ "PipelineRunner cleaned up resources"
   - ❌ If ANY missing: FIX BEFORE PROCEEDING
   
   **⛔ FORBIDDEN:**
   - ❌ Declaring tests passed after only checking START logs
   - ❌ Skipping the `docker logs` check after `pkill`
   - ❌ Assuming shutdown worked without verification

6. ✅ **ONLY THEN commit your changes - IF AND ONLY IF ALL ABOVE PASS**

---

## ❌ ABSOLUTELY FORBIDDEN ACTIONS (Will Result in Invalid Commit):

- ❌ **Committing with ANY failing tests** (Rust unit tests OR Docker integration tests)
- ❌ **Committing with ANY new warnings** (must compare with baseline from previous commit)
- ❌ **Committing without running Docker integration test**
- ❌ **Running `docker-compose up --build` WITHOUT the `-d` flag** (WILL CRASH YOUR CONTEXT)
- ❌ **Assuming tests pass without explicit user confirmation**
- ❌ **Skipping the ask_followup_question step**
- ❌ **Proceeding without checking baseline warnings** (see "How to Check Warnings" section below)

**⚠️ COMMIT CRITERIA (ALL must be true):**
1. ✅ Rust unit tests: ALL PASS, NO new warnings
2. ✅ Docker build: SUCCESSFUL
3. ✅ Stream test: SUCCESSFUL with correct logs
4. ✅ User confirmation: RECEIVED via ask_followup_question

**If ANY of these is false, committing is FORBIDDEN.**

**📋 Violation Handling:**
- Any commit made without following this checklist is INVALID
- User will require you to either:
  - Revert the commit and redo it properly, OR
  - Amend the commit after running proper tests

---

## 🚨 Common AI Mistakes (Learn from History)

**These mistakes have happened. DO NOT repeat them:**

| Mistake | Why It Happened | How to Prevent |
|---------|----------------|----------------|
| **Committing without Docker test** | Only checked cargo test, ignored integration testing | Follow the MANDATORY CHECKLIST above in exact order |
| **Running docker-compose without `-d`** | Didn't read instructions carefully | Copy-paste the exact command: `docker-compose up --build -d` |
| **Context overflow from Docker output** | Tried to monitor build logs directly | ALWAYS use `-d` flag + ask_followup_question pattern |
| **Ignoring new warnings** | Didn't compare with baseline | Always check: `git checkout HEAD~1 && cargo test 2>&1 \| grep warning` |
| **Not using ask_followup_question** | Tried to verify build completion programmatically | Docker requires HUMAN verification, use ask_followup_question |
| **Declaring tests passed without checking stream END logs** | Saw stream start, assumed end works | ALWAYS check logs after `pkill` - stream end must be verified - See step 5 in checklist |
| **Creating git branches without asking user** | Assumed branch name from issue number | ALWAYS ask user which branch to use with ask_followup_question - never assume |

---

## Github issues

- Epic #235
- Sub-issues 236,237,238,239,240,241,242,243

The issues are in the shosho-monorepo github repo. If you do not find the issues you may be in the wrong gh repo. You MUST NOT run `gh repo set-default` or any other command. If you cannot find the issues you must use `ask_question` tool to ask your user for help.

## Github branches

- main - a clean branch state that pre-dates a buggy push by the maintainer of the upstream repo
- safe-working-baseline - a clean branch state that pre-dates a buggy push by the maintainer of the upstream repo as a copy of main in case main becomes broken by future merges from up-stream
- issue-235-cloudflare-backend - the main feature branch which we will be working on
- other sub-feature branches - from time to time we may test other features on some feature branches and then emerge them back into the issue-235 branch

## Build strategies

- The default zap-stream-core repo docker compose pulls from Docker hub
- We need to install build locally in order to test code changes
- Run `docker-compose up --build` to rebuild with changes

## Local Configuration Management (CRITICAL)

### The Problem:
- Files tracked in git history get overwritten during git operations (checkout, pull, merge)
- `.gitignore` only prevents NEW tracking, not replacement of existing tracked files
- We need local config AND local Docker build settings that survive ALL git operations

### The Solution:
**Use Docker Compose override pattern + `.local.yaml` naming for config files**

### How Docker Compose Override Works:
Docker Compose automatically merges (in order):
1. `docker-compose.yaml` (base - tracked in git, gets upstream updates)
2. `docker-compose.override.yaml` (your local changes - NOT in git, never overwritten)

### Implementation:

**Files in Git (can be updated from upstream):**
- `docker-compose.yaml` - Base configuration (pulls from Docker Hub)
- `compose-config.yaml` - Config template (upstream's example)

**Files NOT in Git (your local customizations - protected forever):**
- `docker-compose.override.yaml` - Your local Docker changes (build from source, etc.)
- `compose-config.local.yaml` - Your actual configuration
- `data/` - Docker volumes

### Current Setup:

1. **docker-compose.yaml** (in git):
   ```yaml
   core:
     image: voidic/zap-stream-core  # Default: pull from Docker Hub
     volumes:
       - "./compose-config.yaml:/app/config.yaml"  # Default: use template
   ```

2. **docker-compose.override.yaml** (NOT in git):
   ```yaml
   services:
     core:
       build:  # Override: build from local source
         context: ../..
         dockerfile: crates/zap-stream/Dockerfile
       volumes:
         - "./compose-config.local.yaml:/app/config.yaml"  # Override: use your config
   ```

3. **Protected by .gitignore:**
   ```
   docs/deploy/docker-compose.override.yaml
   docs/deploy/compose-config.yaml  
   *.local.yaml
   ```

### Workflow:

**First time setup (already done):**
```bash
cd zap-stream-core/docs/deploy
cp compose-config.yaml compose-config.local.yaml
# Edit compose-config.local.yaml with your settings
```

**Git Operations Are Now 100% Safe:**
```bash
git checkout any-branch          # Override and .local.yaml untouched
git pull upstream main           # Override and .local.yaml untouched
git merge anything               # Override and .local.yaml untouched
```

**Docker Compose automatically merges:**
```bash
docker-compose up --build
# Merges docker-compose.yaml + docker-compose.override.yaml automatically
# Uses compose-config.local.yaml (from override)
# Builds from local source (from override)
```

### Why This Works:
- `docker-compose.yaml` stays clean, can receive upstream updates
- `docker-compose.override.yaml` is never tracked, never overwritten
- `.local.yaml` files are never tracked, never overwritten
- Git operations are completely safe
- You get automatic upstream updates to base compose file

### Recovery:
If you ever lose your local files:
- `compose-config.local.yaml`: Restore from backup or recreate from `compose-config.yaml` template
- `docker-compose.override.yaml`: Already documented above, just recreate it

## Recommended Git Fork Strategy

### 1. __Create Your Fork__ (One-Time Setup)

First, you need to fork the repository on GitHub:

- Go to [](https://github.com/v0l/zap-stream-core)<https://github.com/v0l/zap-stream-core>
- Click "Fork" button
- Fork it to your organization/account (e.g., `r0d8lsh0p/zap-stream-core` or `shosho/zap-stream-core`)

### 2. __Reconfigure Remotes__ (One-Time Setup)

```bash
cd /Users/visitor/Projects/shosho/zap-stream-core

# Rename current 'origin' to 'upstream'
git remote rename origin upstream

# Add YOUR fork as 'origin'
git remote add origin https://github.com/YOUR_ORG/zap-stream-core.git

# Verify
git remote -v
# Should show:
# origin    https://github.com/YOUR_ORG/zap-stream-core.git (fetch)
# origin    https://github.com/YOUR_ORG/zap-stream-core.git (push)
# upstream  https://github.com/v0l/zap-stream-core.git (fetch)
# upstream  https://github.com/v0l/zap-stream-core.git (push)
```

### 3. __Branching Strategy__

I recommend this structure:

```javascript
main (your production branch)
  ├─ upstream-sync (tracks upstream/main)
  ├─ develop (your integration branch)
  │   ├─ feature/cloudflare-backend (Step 1-3D work)
  │   ├─ feature/step-1-config
  │   ├─ feature/step-2-interface
  │   └─ feature/step-3a-basic-cf
  └─ hotfix/* (emergency fixes)
```

__Branch Purposes:__

- __main__: Your production-ready code, deployed to Railway
- __upstream-sync__: Mirrors upstream/main, never modified directly
- __develop__: Your integration branch where you merge features before production
- __feature/__*: Individual implementation branches per issue

### 4. __Initial Setup Commands__

```bash
# Fetch everything
git fetch --all

# Create upstream-sync branch to track upstream changes
git checkout -b upstream-sync upstream/main
git push -u origin upstream-sync

# Create your main branch (if starting fresh)
git checkout -b main upstream/main
git push -u origin main

# Create develop branch
git checkout -b develop
git push -u origin develop
```

### 5. __Day-to-Day Workflow__

__Starting a new feature__ (e.g., Step 1 - Configuration):

```bash
git checkout develop
git pull origin develop
git checkout -b feature/step-1-config
# ... make changes ...
git add .
git commit -m "Add backend configuration structure (closes #236)"
git push -u origin feature/step-1-config
# Create PR: feature/step-1-config → develop
```

__Merging to develop__:

```bash
# After PR approval
git checkout develop
git pull origin develop
git merge --no-ff feature/step-1-config
git push origin develop
```

__Promoting to production__:

```bash
# When ready to deploy
git checkout main
git pull origin main
git merge --no-ff develop
git tag -a v1.0.0-cloudflare -m "Cloudflare backend implementation"
git push origin main --tags
```

### 6. __Syncing from Upstream__ (Regular Activity)

This is the critical part for getting upstream changes:

```bash
# 1. Update your upstream-sync branch
git checkout upstream-sync
git fetch upstream
git merge upstream/main
git push origin upstream-sync

# 2. Decide what to merge into your codebase
#    Option A: Merge everything
git checkout develop
git merge upstream-sync
# ... resolve conflicts if any ...
git push origin develop

#    Option B: Cherry-pick specific commits
git checkout develop
git log upstream-sync  # Find commits you want
git cherry-pick <commit-hash>
git push origin develop
```

### 7. __Conflict Resolution Strategy__

When upstream changes conflict with your Cloudflare work:

__Your advantages:__

- Your abstraction layer (Step 2) isolates changes

- Core business logic stays the same

- Most conflicts will be in:

  - `main.rs` (listener setup)
  - `overseer.rs` (if they change business logic)
  - Cargo.toml (dependencies)

__Resolution approach:__

```bash
git checkout develop
git merge upstream-sync
# Conflicts appear

# For each conflict:
# 1. Understand what upstream changed and WHY
# 2. Integrate their change into your abstraction
# 3. Test both backends still work

git add .
git commit -m "Merge upstream changes, integrate with backend abstraction"
```

### 8. __When to Pull from Upstream__

__High Priority__ (merge immediately):

- Security fixes
- Critical bug fixes
- Database schema changes

__Medium Priority__ (review and merge in next cycle):

- Performance improvements
- New features that don't conflict
- Refactoring that improves code quality

__Low Priority__ (evaluate if needed):

- New features you don't use
- Alternative implementations of what you already have

### 9. __Communication with Upstream__

__Consider contributing back:__ If your abstraction layer is clean, you could:

1. Submit a PR to upstream with the abstraction interface (Step 2)

2. Keep RmlRtmpBackend as their implementation

3. Keep CloudflareBackend in your fork

4. Benefits:

   - Upstream maintains compatibility with your abstraction
   - Others can implement other backends (AWS IVS, etc.)
   - Less conflict resolution for you

__Suggested approach:__

```bash
# After Step 2 is complete and tested
git checkout -b upstream-contribution
git rebase -i develop  # Clean up commits
# Only include Step 1 & 2 changes
git push origin upstream-contribution
# Create PR to v0l/zap-stream-core
```

### 10. __Protection Rules__

Set up branch protection on your fork:

- __main__: Require PR reviews, require CI to pass
- __develop__: Require CI to pass
- __upstream-sync__: Prevent direct pushes (only fast-forwards from upstream)

## Example Timeline Scenario

__Month 1__: You implement Steps 1-3D

- Work in `develop` branch
- Deploy to staging from `develop`

__Month 2__: Upstream adds new payment provider

- Changes conflict with your overseer.rs

- Your process:

  ```bash
  git checkout upstream-sync
  git pull upstream main
  git checkout develop
  git merge upstream-sync
  # Conflict in overseer.rs
  # Resolve by adding payment provider to abstraction
  # Test both RTMP and Cloudflare backends work
  git commit
  ```

__Month 3__: Deploy to production

- Merge `develop` → `main`
- Deploy `main` to Railway
- Tag as `v1.0.0-cloudflare`

## Recommended .gitignore Additions

Add to zap-stream-core/.gitignore:

```javascript
# Cloudflare-specific configs (don't commit credentials)
config.cloudflare.yaml
.env.cloudflare

# Local testing
test-streams/
*.local.yaml
```

## Summary

__Key Strategy:__

1. __origin__ = your fork (where you push)
2. __upstream__ = v0l's repo (where you pull updates from)
3. __upstream-sync__ branch = pure mirror of upstream
4. __develop__ branch = your integration point
5. __feature/__ branches = individual issues
6. __main__ branch = production deployment

__Merge Flow:__

```javascript
upstream/main → upstream-sync → develop → main → Railway
                    ↑              ↑
                  (pull)        (merge)
```

This strategy gives you:

- ✅ Clear separation from upstream
- ✅ Ability to pull updates anytime
- ✅ Isolated feature development
- ✅ Clean deployment path
- ✅ Option to contribute back

## 📊 How to Check Warnings (DEFINITIVE METHOD)

**⚠️ WARNING CONFUSION ALERT:** Multiple AIs have gotten confused about warning counts. Use this ONE method only.

### The ONE Correct Method:

**Step 1: Get baseline warning count:**
```bash
cd /Users/visitor/Projects/shosho/zap-stream-core
git stash  # Save your changes temporarily
git checkout HEAD~1  # Go to previous commit
cargo test 2>&1 | grep "generated.*warning"
```

**You'll see output like:**
```
warning: `zap-stream-core` (lib) generated 1 warning
warning: `zap-stream-core` (lib test) generated 3 warnings (1 duplicate)
warning: `zap-stream` (bin "hls_debug" test) generated 2 warnings
warning: `zap-stream` (bin "zap-stream" test) generated 7 warnings
```

**Step 2: Remember the baseline numbers:**
- zap-stream-core (lib): 1
- zap-stream-core (lib test): 3 (1 duplicate)
- zap-stream (hls_debug): 2
- zap-stream (zap-stream): 7

**Step 3: Return to your branch:**
```bash
git checkout -  # Return to your branch
git stash pop  # Restore your changes
```

**Step 4: Check your branch's warning count:**
```bash
cargo test 2>&1 | grep "generated.*warning"
```

**Step 5: Compare the numbers:**
- ✅ **PASS**: All numbers are the same or lower
- ❌ **FAIL**: ANY number is higher (you introduced new warnings)

### Example of PASS:
```
Baseline: zap-stream (zap-stream) generated 7 warnings
Your code: zap-stream (zap-stream) generated 7 warnings
✅ SAME - No new warnings introduced
```

### Example of FAIL:
```
Baseline: zap-stream (zap-stream) generated 7 warnings
Your code: zap-stream (zap-stream) generated 8 warnings
❌ NEW WARNING - Must fix before committing
```

### ⚠️ Common Mistakes to Avoid:

1. ❌ **WRONG**: Counting with `grep "^warning:" | wc -l` → gives wrong numbers (18 vs 7)
2. ❌ **WRONG**: Only looking at ONE package's warnings → misses warnings in other packages
3. ❌ **WRONG**: Comparing with `main` instead of `HEAD~1` → doesn't show YOUR changes
4. ✅ **CORRECT**: Use `grep "generated.*warning"` and compare ALL packages

### Why This Method Works:

- **Per-package breakdown**: Shows exactly which package has warnings
- **Handles duplicates**: Rust compiler tells you when warnings are duplicates
- **Consistent**: Same command always gives same format
- **Clear pass/fail**: Easy to see if numbers changed

### If You Introduced New Warnings:

1. Read the detailed warning messages in the test output
2. Fix the code to eliminate the warnings
3. Re-run `cargo test 2>&1 | grep "generated.*warning"`
4. Verify the warning count matches baseline
5. Only then proceed with commit

Alternatively, explain to your user why an exception should be permitted.

---

## ⚠️ CRITICAL: Testing Requirements (Required Before Every Commit)

**YOU MUST RUN BOTH TESTS BEFORE ANY COMMIT - NO EXCEPTIONS**

### Why Both Tests Matter:
- **Rust unit tests** verify code compiles and logic is correct
- **Docker integration tests** verify the actual streaming system works end-to-end
- Missing either test catches only half the problems
- Every commit must pass BOTH tests

### Test Order:
1. **First**: Run Rust unit tests (fast feedback on code issues)
2. **Second**: Run Docker integration test (verifies full system works)
3. **Only then**: Commit and push

---

## Testing Strategy 0 - Rust Unit Tests (Always Run First)

### Purpose:
Verify that:
- Code compiles without errors
- Unit tests pass
- Type system is satisfied
- No obvious logic errors

### How to Run:

```bash
cd /Users/visitor/Projects/shosho/zap-stream-core
cargo test
```

### Expected Output:
- All tests pass (green)
- No compilation errors
- Warnings are acceptable but should be reviewed

### Timing:
- **First run**: 5-10 minutes (downloads dependencies, compiles everything)
- **Subsequent runs**: 5-10 seconds (only recompiles changed code)

### What This Does NOT Test:
- Docker configuration
- RTMP streaming functionality
- Database migrations
- Actual end-to-end workflows

**After cargo test passes, you MUST run the Docker integration test below.**

---

## Testing Strategy 1 - Docker Integration Test (Always Run Second)

### IMPORTANT: Running Docker Build Without Overwhelming Context

**Problem:** Docker build output is massive and will overflow AI context window, causing system crash.

**Solution:** Use detached mode (`-d`) and ask_followup_question tool.

**Workflow for AI Assistants:**

```bash
# Step 1: Start Docker build in background (returns immediately)
cd /Users/visitor/Projects/shosho/zap-stream-core/docs/deploy
docker-compose up --build -d
```

**Step 2: Use ask_followup_question to wait for user confirmation:**
```
Question: "Docker build started in background. Please let me know when the containers are fully built and running (check with `docker ps` or Docker Desktop)."
Options: ["Build completed successfully", "Build failed - see error"]
```

**Step 3: Only after user confirms build completion, proceed with testing.**

**DO NOT attempt to:**
- Run `docker-compose up --build` without `-d` flag (will overflow context)
- Monitor build logs with `docker-compose logs -f` (will overflow context)
- Wait programmatically for build completion (no reliable way without logs)

---

### Database Setup

**Test User for Streaming:**
- **User ID**: 55
- **Pubkey**: `9b8929f0ddefc96c9eb70dff17eec27826277acee7b6536fcf843b592fad793c`
- **Stream Key**: `81b97dd0-b959-11f0-b22c-d690ca11bae8`
- **Balance**: 0 (works with free "Basic" endpoint)

**Query to get user info:**
```sql
docker exec -it zap-stream-core-db-1 mariadb -uroot -proot zap_stream -e \
  "SELECT id, HEX(pubkey) as pubkey_hex, stream_key, balance FROM user WHERE id=55;"
```

### Ingest Endpoints

Two endpoints are configured by default (from migration `20250919101353_add_defualt_endpoints.sql`):

| Endpoint | Cost (millisats/min) | Capabilities | Use Case |
|----------|---------------------|--------------|----------|
| **Basic** | 0 | variant:source | **Free tier** - streams source quality only |
| **Good** | 2500 | Multiple variants | Paid tier - transcodes to multiple qualities |

**Query to view endpoints:**
```sql
docker exec -it zap-stream-core-db-1 mariadb -uroot -proot zap_stream -e \
  "SELECT * FROM ingest_endpoint;"
```

### RTMP URL Format

The RTMP URL requires **two path components**:

```
rtmp://localhost:1935/{ENDPOINT_NAME}/{STREAM_KEY}
```

**Components:**
- `ENDPOINT_NAME`: Must match an endpoint name in the database (case-insensitive)
- `STREAM_KEY`: The user's stream key from the `user` table

**Example (Free Basic Endpoint):**
```
rtmp://localhost:1935/Basic/81b97dd0-b959-11f0-b22c-d690ca11bae8
```

### Test Streaming with ffmpeg

**Start a test stream:**
```bash
ffmpeg -re \
  -f lavfi -i testsrc=size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=1000:sample_rate=44100 \
  -c:v libx264 -preset veryfast -tune zerolatency \
  -c:a aac -ar 44100 \
  -f flv rtmp://localhost:1935/Basic/81b97dd0-b959-11f0-b22c-d690ca11bae8 \
  </dev/null >/dev/null 2>&1 &
```

**Verify in Docker logs:**
```bash
docker logs --tail 50 --follow zap-stream-core-core-1
```

**Expected success logs for streaming start:**
```
INFO zap_stream_core::ingress::rtmp: Published stream request: Basic/81b97dd0-b959-11f0-b22c-d690ca11bae8 [Live]
INFO zap_stream_core::pipeline::runner: Pipeline run starting
INFO zap_stream::overseer: Published stream event [event_id]
INFO zap_stream_core::mux::hls::variant: Created fMP4 initialization segment
INFO zap_stream::overseer: Checking stream is alive: [stream_id]
```

**Stop the test stream:**
```bash
pkill -9 -f "ffmpeg.*testsrc"
```

**Expected success logs for streaming end:**
```
read_data EOF
WARN zap_stream_core::pipeline::runner: Demuxer get_packet failed, entering idle mode
INFO zap_stream_core::pipeline::runner: Switched to idle mode - generating placeholder content
WARN zap_stream::overseer: Stream [stream_id] timed out - no recent segments
INFO zap_stream::overseer: Stream ended [stream_id]
INFO zap_stream_core::pipeline::runner: Idle timeout reached (60 seconds), ending stream
ERROR zap_stream_core::pipeline::runner: Pipeline run failed error=Idle timeout reached
INFO zap_stream_core::pipeline::runner: PipelineRunner cleaned up resources for stream: [stream_id]
```

### Common Errors and Solutions

**Error: "Invalid app or key"**
- **Cause**: Missing app name in RTMP URL
- **Wrong**: `rtmp://localhost:1935/81b97dd0-b959-11f0-b22c-d690ca11bae8`
- **Correct**: `rtmp://localhost:1935/Basic/81b97dd0-b959-11f0-b22c-d690ca11bae8`

**Error: "Not enough balance"**
- **Cause**: User has balance=0 and endpoint has cost > 0
- **Solution**: Either:
  - Use "Basic" endpoint (cost=0)
  - Add balance to user: `UPDATE user SET balance = 10000000 WHERE id = 55;`

**Error: "User not found or invalid stream key"**
- **Cause**: Stream key doesn't exist in database
- **Solution**: Query database to verify correct stream key


## New Cloudflare Backend Integration SHOULD be testable with end to end integration test 

But AI sessions have been trying and failing to test it successfully

AI failures included

- testing webhooks only, but not testing streaming (faled e2e)
- testing streaming only, but not testing webhooks (faled e2e)
- making up fake cryptography rather than using actual Nostr NIP-98 header auth (failed to run)

They often blamed incomplete architecture, but then changed their mind and said it was complete

They often used attempt_completion tool to try to say that tests that did not work were proof of successful implementaiton, which is braindead.

There are at least two potentially junk, legacy cruft e2e test implementations in the /scripts folder

The most recent failed AI session wrote this as its explainer, and it may actually be true, but until tests pass is invalid.

```markdown
## Cloudflare Integration Testing - Critical Requirements

### What Must Be Proven
A working integration test MUST demonstrate the COMPLETE lifecycle:
1. API call with NIP-98 auth creates Live Inputs
2. FFmpeg streams to Cloudflare RTMP endpoint
3. **Cloudflare sends webhook to our server** (stream connected)
4. **Webhook triggers database updates and Nostr publishing**
5. Stream ends
6. **Cloudflare sends disconnect webhook**
7. **Webhook triggers cleanup**

### Why Previous Attempts Failed
- Streaming to Cloudflare works, but webhooks require Cloudflare Tunnel
- The `cloudflared` service in docker-compose has profile "cloudflared" (not running by default)
- Without the tunnel, Cloudflare cannot reach the local server
- Testing showed NO webhooks arriving = NO lifecycle proven

### Prerequisites for Testing
1. Start cloudflared tunnel: `docker-compose --profile cloudflared up -d`
2. Verify tunnel is accessible from internet
3. Configure webhook URL in config to point to tunnel
4. THEN run integration tests
5. Confirm webhooks arrive in Docker logs

### The NIP-98 Signer
- Located at `scripts/sign_nip98.js`
- Uses nostr-tools library (proper cryptography, not handrolled)
- Usage: `node sign_nip98.js <nsec> <url> <method>`
- This component works correctly
```

That's what a future AI needs to know: __The tunnel is the missing piece__.

### Things that I expect to see in a working end to end test

1. a user can successfully get their stream key
2. they can successfully stream to that key
3. cloudflare successfully receives their stream
4. cloudflare successfully notifies the webhook of stream start
5. the webhook successfully triggers the stream start workflow
6. this is evidenced in the logs
7. the hls can be played back for example at localhost
8. a user can successfully end their stream
9. cloudflare successfully stops receiving their stream
10. cloudflare successfully notifies the webhook of stream stop
11. the webhook successfully triggers the stream stop workflow
12. this is evidenced in the logs
13. the hls can no longer be played back for example at localhost
