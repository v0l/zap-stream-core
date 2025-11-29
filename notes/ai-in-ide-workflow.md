# 🧠 AI‑in‑IDE Brief: GitHub‑Driven Workflow

## Workflow Rules

1. **Open an Issue** for every new feature or bug (“story”).
2. **Break work down** inside the issue  
   * Use Markdown check‑lists (`- [ ] …`) for micro‑tasks
   * GitHub renders these as clickable checkboxes in the web UI
   * Reference issues in checkboxes with `- [ ] #123` syntax
   * When referenced issues close, GitHub auto-ticks the checkbox
   * If a task grows, use **"Convert to issue"** in GitHub's UI to create a linked child issue
   * When the child issue closes, the parent checkbox auto-ticks
3. **Assign the issue and track progress**  

   ```bash
   # Assign the issue to yourself
   gh issue edit <number> --add-assignee "@me"
   ```

4. **Branch per issue**  

   ```bash
   git checkout -b issue-<number>-short-description
   ```

5. **Commit message must reference the issue**  

   ```bash
   git commit -am "Add /ping endpoint (closes #42)"
   ```

6. **Push & open PR with proper description**  

   ```bash
   # Push the branch
   git push -u origin issue-<number>-short-description
   
   # Create PR with descriptive title and body
   # EXAMPLE: For a PR implementing issue #42 for a /ping endpoint:
   gh pr create --title "Add /ping endpoint" --body "Resolves #42
   
   This PR adds a /ping endpoint for health checks with the following features:
   - Simple response with 200 status code
   - Response includes server timestamp
   - Documented in API docs"
   ```
   
   > **Note for AI assistants**: When creating a PR, extract the relevant details from:
   > 1. The issue description and checklist items
   > 2. The actual code changes made
   > 3. Always include "Resolves #<issue-number>" to link the PR to the issue

7. **Post-Merge Cleanup**  

   After a PR is approved and merged:

   ```bash
   # Check if you're on main branch, if not switch to it
   git branch | grep "* main" || git checkout main
   
   # Pull the latest changes to get the merged code
   git pull origin main
   
   # Verify the changes are present (e.g., check the file that was modified)
   # use your read file tool to check e.g. path/to/modified/file.js
   
   # Check if the issue branch still exists locally
   git branch | grep "issue-<number>-"
   
   # Check if the issue branch still exists remotely
   git ls-remote --heads origin | grep "issue-<number>-"
   
   # Delete the branch locally if it exists
   git branch -D issue-<number>-short-description
   
   # Delete the branch remotely if it exists
   git push origin --delete issue-<number>-short-description
   
   # Remove any temporary markdown files created during development
   git rm <temporary_file>.md
   git commit -m "Remove temporary development notes"
   git push origin main
   ```
   
   > **Note for AI assistants**: 
   > 1. Always return to main branch after a PR is merged
   > 2. Pull the latest changes to ensure you have the merged code
   > 3. Verify that the expected changes are present
   > 4. Clean up by deleting the feature branch
   > 5. **Remove any temporary .md files** you created during development (e.g., notes, drafts, etc.)
   > 6. This keeps the repository clean and ensures you start new work from the latest code

8. **Milestones = release buckets**  
   Assign issues to `0.8.0`, `0.9.0` … to track progress toward each tag.

## Status Tracking

* Plain check‑list items → _unticked / ticked_ (Todo / Done).  
* Issues gain **Todo → In progress → Done** when added to a GitHub Projects board.  
* Add an **Iteration** field later if you switch to sprints.

---

## Appendix 1: Task Tracking for AI Assistants

### Exact Commands for Issue Management

1. **Assigning Issues to Yourself**
   ```bash
   # Replace <number> with the issue number
   cd /path/to/repo && gh issue edit <number> --add-assignee "@me"
   
   # Example:
   cd /Users/visitor/Projects/shosho/shosho-monorepo && gh issue edit 19 --add-assignee "@me"
   ```

2. **Updating Checkboxes via CLI**

   **Method A: Three-Command Approach (with temp file)**
   
   | # | Command | Purpose |
   |---|---------|---------|
   | 1 | `gh issue view <number> --json body -q .body > body.md` | Download the issue body into a temporary file |
   | 2 | `sed -i '' 's/- \[ \] Task A/- \[x\] Task A/' body.md` (macOS)<br>or<br>`sed -i 's/- \[ \] Task A/- \[x\] Task A/' body.md` (Linux) | Edit the file: replace unchecked box with checked box |
   | 3 | `gh issue edit <number> --body-file body.md` | Upload the modified body back to GitHub |
   
   **Example (using issue #19):**
   ```bash
   # Working directory: /Users/visitor/Projects/shosho/shosho-monorepo
   
   # 1. Download issue body
   gh issue view 19 --json body -q .body > body.md
   
   # 2. Update checkbox (macOS)
   sed -i '' 's/- \[ \] Project overview and purpose/- \[x\] Project overview and purpose/' body.md
   
   # 3. Upload changes
   gh issue edit 19 --body-file body.md
   ```

   **Method B: In-Memory Approach (recommended)**
   
   This approach avoids creating temporary files by using shell variables:
   
   ```bash
   # Working directory: /Users/visitor/Projects/shosho/shosho-monorepo
   
   # All-in-one command:
   body=$(gh issue view 19 --json body -q .body)
   new_body=$(printf '%s\n' "$body" | sed 's/- \[ \] Project overview and purpose/- \[x\] Project overview and purpose/')
   gh issue edit 19 --body "$new_body"
   ```
   
   > **Note**: This is just a simple text replacement operation - no complex scripting required. Either pattern works effectively to update checkboxes via CLI.

3. **Creating Sub-issues for Automatic Checkbox Updates**
   ```bash
   # Create a sub-issue that references the parent issue
   cd /path/to/repo && gh issue create --title "Subtask: Feature X" --body "Part of #<parent-number>"
   
   # Update the parent issue to reference the sub-issue in a checkbox
   cd /path/to/repo && gh issue view <parent-number> --json body -q .body > parent_body.md
   # Add a line like: "- [ ] #<sub-issue-number> Implement Feature X"
   cd /path/to/repo && gh issue edit <parent-number> --body-file parent_body.md
   
   # When the sub-issue is closed, the checkbox will auto-tick
   ```

4. **Verifying Changes**
   ```bash
   # Always verify your changes after updating an issue
   cd /path/to/repo && gh issue view <number>
   ```

> **Important**: Always use absolute paths or ensure you're in the correct directory before running commands. Check your current directory with `pwd` if unsure.

## Appendix 2: AI Assistant Anti-Patterns to Avoid

When working with this GitHub-driven workflow, AI assistants should avoid these common pitfalls:

### 🚫 CRITICAL: NEVER CREATE PULL REQUESTS

**❌ ABSOLUTELY FORBIDDEN**: Using `gh pr create` command or any method to create pull requests.

**✅ CORRECT approach**: 
- Push your branch to GitHub: `git push origin <branch-name>`
- **STOP THERE** - The user will create the PR when they decide the work is ready
- Never assume work is ready for PR - that is the user's decision

**Why this rule exists**:
- The user decides when or if to create a PR
- You will often be called to commit multiple times in the course of work
- You should not expect that you are fully aware of the scope of future work
- Creating PRs prematurely angers the user as he must then delete them
- The user knows when the implementation is truly complete

### 🚫 CRITICAL: NEVER USE GIT ADD WITH SPECIFIC FILENAMES

**❌ ABSOLUTELY FORBIDDEN**: Using `git add file1.ts file2.ts` or listing specific files.

**✅ CORRECT approach**: ALWAYS use `git add .` to stage all changes.

**Why this rule exists**:
- Easy to accidentally miss files (e.g., `yarn.lock`, `.yarnrc.yml`, package files)
- Missing dependency files causes build failures (Vercel, CI/CD)
- Using `git add .` ensures all related changes are included
- The `.gitignore` file already handles what should NOT be committed

**Examples of what gets missed**:
- `yarn.lock` or `package-lock.json` after installing dependencies
- `.yarnrc.yml` configuration changes
- Auto-generated files that are needed for builds
- Hidden configuration files

**Correct workflow**:
```bash
# After making changes and testing
git add .                    # Stage ALL changes (respects .gitignore)
git commit -m "Your message"
git push origin <branch-name>
```

### 1. Simulation Instead of Implementation

**❌ Anti-pattern**: Attempting to simulate the workflow without actually executing the required commands.

**✅ Correct approach**: Execute the actual git commands and GitHub CLI commands as specified in the workflow. If tools are missing (e.g., GitHub CLI), help install them rather than simulating.

### 2. Focusing on Content Before Workflow

**❌ Anti-pattern**: Jumping straight to creating content (e.g., writing a README) before establishing the proper workflow context (issue, branch, etc.).

**✅ Correct approach**: Follow the workflow steps in order - create issue first, then branch, then implement changes, then commit with reference, then push and create PR.

### 3. Overcomplicating the Process

**❌ Anti-pattern**: Adding unnecessary steps or theoretical discussions instead of following the concrete workflow steps.

**✅ Correct approach**: Stick to the specific workflow steps outlined in this document. Each step has a clear purpose in the GitHub-driven workflow.

### 4. Ignoring Prerequisites

**❌ Anti-pattern**: Attempting to use tools without checking if they're installed or properly configured.

**✅ Correct approach**: Verify tool availability (git, GitHub CLI) before proceeding, and help set up missing components.

### 5. Abandoning the Workflow When Facing Obstacles

**❌ Anti-pattern**: Switching to an ad-hoc approach when encountering issues with the workflow.

**✅ Correct approach**: When obstacles arise, address them directly while maintaining the workflow structure. For example, if GitHub CLI is not available, help install it rather than abandoning the workflow.

### 6. Running Commands in the Wrong Directory

**❌ Anti-pattern**: Running git or GitHub CLI commands without verifying the current working directory.

**✅ Correct approach**: Always ensure you are in the right directory before executing commands:
   - **ALWAYS USE ABSOLUTE PATHS for GitHub CLI commands to ensure correct directory**
   - **CRITICAL FOR GITHUB COMMANDS**: GitHub CLI commands (gh) must be run from the repository root directory:
     ```bash
     # CORRECT: Always use absolute paths for GitHub commands
     cd /Users/visitor/Projects/shosho/shosho-monorepo && gh issue comment 40 --body "Your comment here"
     
     # INCORRECT: Using relative paths or no path prefix - these may fail
     cd shosho-monorepo && gh issue comment 40 --body "Your comment here"
     gh issue comment 40 --body "Your comment here"
     ```

### 7. Leaving Temporary Files in the Repository

**❌ Anti-pattern**: Creating temporary markdown files for notes, drafts or comments during development but forgetting to remove them before completing the task.

**✅ Correct approach**: Clean up all temporary files after they've served their purpose:
   - Remove temporary .md files with `git rm file.md` once their content has been copied to GitHub issues/PRs
   - Commit the removal with a clear message: `git commit -m "Remove temporary development notes"`
   - Push the cleanup commit to keep the repository tidy: `git push origin main`

Remember: The purpose of this workflow is to maintain a clear connection between issues, code changes, and pull requests. Each step reinforces this connection and should not be skipped or substituted.

## Appendix 3: GitHub Comment and Issue Creation Best Practices

When posting GitHub comments or creating issues using `gh issue comment` or `gh issue create` commands, be careful with how code blocks and backticks are handled. The shell may interpret backticks (`) in the markdown as command substitution in bash/zsh, which can lead to errors like:

```
zsh: command not found: packageName
zsh: permission denied: path/to/file.tsx
```

### Best Practices for Posting Comments and Creating Issues with Code Blocks

**WARNING: These practices apply to BOTH `gh issue comment` AND `gh issue create` commands. Failure to follow these practices for either command will result in shell interpretation errors.**

#### Method 1: Using the `--body` Parameter (NOT Recommended for Content with Code Blocks)

The `--body` parameter can be used for simple comments or issues without code blocks:

```bash
# For comments:
gh issue comment <issue-number> --body "Simple text without code blocks."

# For creating issues:
gh issue create --title "Simple Issue" --body "This issue doesn't contain any code blocks or special characters."
```

**IMPORTANT:** This approach is NOT reliable for content with code blocks, backticks, or other special characters! Use Method 2 instead for any content with code, special formatting, or multi-line text.

#### Method 2: Using a Body File (RECOMMENDED for All Content with Code Blocks)

For comments or issues with code blocks, complex formatting, or multiple lines:

```bash
# 1. Create a temporary markdown file with your content
cat > content.md << 'EOL'
# Title

Normal text here.

```typescript
function example() {
  return true;
}
```

Additional explanation here.
EOL

# 2. Use the file for your comment or issue
# For comments:
gh issue comment <issue-number> --body-file content.md

# For creating issues:
gh issue create --title "Issue with Code Examples" --body-file content.md

# 3. Clean up the temporary file
rm content.md
```

**Note the use of single quotes around 'EOL'** - this prevents shell interpretation of the content between the heredoc markers.

#### Method 3: Escaping Backticks (For Very Simple Cases Only)

If you must use inline commands with backticks, escape them:

```bash
gh issue comment <issue-number> -b "Code example: \`const x = 5;\`"
```

This approach is only recommended for very simple cases with minimal special characters.

### Verifying Terminal Output After Commands

**CRITICAL:** After running any GitHub CLI command:

1. **ALWAYS check the terminal output** to confirm the command executed successfully
2. Look for specific error messages that might indicate:
   - Shell interpretation errors
   - Authentication issues
   - API errors
3. If errors occur, fix them before proceeding to the next step
4. Pay special attention to commands that create or modify content

Example error verification workflow:

```bash
# 1. Execute the command
gh issue create --title "My Issue" --body-file issue_content.md

# 2. Check terminal output for success or errors
# Success usually shows the URL of the created resource
# Example success: https://github.com/user/repo/issues/42

# 3. Only proceed to the next step if the command was successful
# If an error occurred, fix it and try again
```

This verification step is mandatory for all GitHub CLI operations to ensure the workflow progresses correctly.

## Appendix 4: Shell Warning and GitHub Operation Verification Rules

### CRITICAL RULE: Never Ignore Shell Warnings

**❌ NEVER DO THIS**: Assume a GitHub operation succeeded when shell warnings or errors appear in the terminal output.

**✅ ALWAYS DO THIS**: 
1. **Read and analyze ALL terminal output** after every GitHub CLI command
2. **Investigate any warnings or errors** before proceeding
3. **Verify the operation actually completed** by reading the created/updated content

### Shell Warning Examples That Must Not Be Ignored

Common shell warnings that indicate problems:

```bash
# Example 1: Character escaping issues
zsh: no matches found: [Broadcast]
zsh: permission denied: //

# Example 2: Command substitution errors  
zsh: command not found: packageName
bash: syntax error near unexpected token

# Example 3: File permission issues
zsh: permission denied: /path/to/file
```

### Mandatory Verification Steps for GitHub Operations

After ANY GitHub CLI command that creates or modifies content:

1. **Check the terminal output for the success URL**
   ```bash
   # Success example:
   https://github.com/user/repo/issues/47#issuecomment-3130282555
   
   # If you see this URL, the operation likely succeeded
   # But you must still verify the content (step 2)
   ```

2. **Read the actual created/updated content to verify it matches your intent**
   ```bash
   # For issue comments:
   gh issue view <issue-number>
   
   # For new issues:
   gh issue view <issue-number>
   
   # For PR comments:
   gh pr view <pr-number>
   ```

3. **Compare the actual content with what you intended to post**
   - Check that code blocks rendered correctly
   - Verify that special characters weren't mangled
   - Ensure the formatting is as expected

### Example of Proper Verification Workflow

```bash
# 1. Execute the command
gh issue comment 47 --body-file comment.md

# 2. Check terminal output
# Look for success URL: https://github.com/user/repo/issues/47#issuecomment-XXXXXX
# Look for any warnings or errors

# 3. If there were ANY warnings, investigate them
# Even if a URL was returned, warnings may indicate partial failure

# 4. Verify the actual content
gh issue view 47

# 5. Read through the comment content to ensure it matches your intent
# Pay special attention to:
# - Code blocks (should be properly formatted)
# - Special characters (should not be mangled)
# - Line breaks and formatting
```

### When GitHub Operations Fail

If verification reveals the operation failed or content is incorrect:

1. **Identify the root cause** (usually shell interpretation of special characters)
2. **Fix the content** using proper escaping or body files
3. **Re-execute the command** with the corrected approach
4. **Re-verify** the results

**Remember**: A URL in the terminal output does NOT guarantee the content is correct. Always verify by reading the actual created/updated content.

## ⛔ CRITICAL ANTI-PATTERN: "Doesn't need testing" Fallacy ⛔

### ❌ ABSOLUTELY FORBIDDEN REASONING:

**NEVER say or think:**
- ❌ "This is just a documentation change, no testing needed"
- ❌ "I only modified markdown files, skip tests"
- ❌ "Comments don't affect code, no need to test"
- ❌ "This is a minor change, testing is overkill"

### ✅ THE TRUTH:

**EVERY push triggers Railway deployment:**
1. E.g. if you push to `issue-yxz-feature-name`
2. Railway and Vercel detect the push
3. They both attempt to build and run the push
4. **If build/run fails, the environment breaks!**

**This happens for ALL pushes:**
- ✅ Code changes = Railway and Vercel deploy
- ✅ Documentation changes = Railway and Vercel deploy
- ✅ Comment changes = Railway and Vercel deploy
- ✅ README changes = Railway and Vercel deploy
- ✅ ANY file change = Railway and Vercel deploy

### The Rule Is Absolute:

**Before ANY commit/push:**
1. Run all tests
2. ALL tests must pass
3. NO exceptions for "documentation only"
4. NO exceptions for "minor changes"
5. NO exceptions EVER

**Why this matters:**
- A bad push breaks deployment
- Deployment failure = environment downtime
- Testing catches build issues BEFORE they reach Railway
- "Just documentation" is never just documentation

### If You Think "This Doesn't Need Testing":

**STOP. You are wrong. Test anyway.**