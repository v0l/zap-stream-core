# Cloudflare API Discovery Script

## Purpose

This script tests the actual Cloudflare Stream API to discover the true response structure, specifically:

- Where is the HLS playback URL located?
- Does Live Input have a `created` field with Video asset UID?
- What fields are available during vs before streaming?
- How does the API actually behave in production?

## Prerequisites

### 1. Cloudflare Account

You need a Cloudflare account with Stream enabled:

1. Go to https://cloudflare.com
2. Sign up or log in
3. Navigate to **Stream** in the dashboard
4. Free tier includes 1,000 minutes/month (plenty for testing)

### 2. Get API Credentials

**Account ID:**
1. Go to Cloudflare dashboard
2. Select your domain
3. Scroll down on the right sidebar
4. Copy your Account ID

**API Token:**
1. Go to https://dash.cloudflare.com/profile/api-tokens
2. Click "Create Token"
3. Use "Edit Cloudflare Stream" template OR create custom with:
   - Permissions: `Stream:Write` and `Stream:Read`
   - Account Resources: Include your account
4. Click "Continue to summary"
5. Click "Create Token"
6. **Copy the token immediately** (you won't see it again!)

### 3. Required Tools

- `curl` - for API calls (installed by default on macOS/Linux)
- `jq` - for JSON parsing
  - macOS: `brew install jq`
  - Linux: `apt-get install jq` or `yum install jq`
- `ffmpeg` - for optional streaming test (already used in Docker tests)

## Running the Script

### 1. Create .env File (Recommended)

```bash
cd /Users/visitor/Projects/shosho/zap-stream-core/scripts
cp .env.example .env
```

Then edit `.env` with your actual credentials:
```bash
# Edit with your favorite editor
nano .env
# or
code .env
```

**The .env file will NOT be committed to git** (already in .gitignore)

**Alternative: Export Variables Directly**
```bash
export CLOUDFLARE_ACCOUNT_ID="your-account-id-here"
export CLOUDFLARE_API_TOKEN="your-api-token-here"
```

### 2. Navigate to Scripts Directory (if not already there)

```bash
cd /Users/visitor/Projects/shosho/zap-stream-core/scripts
```

### 3. Run the Script

```bash
./cloudflare-api-discovery.sh
```

## What the Script Does

### Automatic Tests (No User Input Required)

1. **Create Live Input** - POSTs to Cloudflare API
2. **Get Live Input Details** - GETs the created resource
3. **List All Live Inputs** - Verifies list endpoint
4. **Analysis** - Checks for HLS URLs and Video assets

All responses are saved to timestamped directory for later review.

### Optional Streaming Test

The script will pause and display an ffmpeg command. You can:

**Option A: Skip (Press ENTER)**
- Continues without testing active streaming
- Good for quick API structure check

**Option B: Test with Active Stream**
1. Open a new terminal
2. Copy-paste the displayed ffmpeg command
3. Wait 30 seconds for stream to start
4. Return to first terminal and press ENTER
5. Script will check API response while streaming

### Cleanup

Script will ask if you want to delete the test Live Input:
- **Yes**: Cleans up immediately
- **No**: Leaves it for manual cleanup (command provided)

## Output

### Directory Structure

```
cloudflare-api-responses-YYYYMMDD_HHMMSS/
├── 1-create-live-input.json          # POST response
├── 2-get-live-input-before-stream.json  # GET before streaming
├── 3-list-live-inputs.json            # List endpoint
├── 4-get-live-input-while-streaming.json  # GET during/after stream
├── 5-get-video-asset.json             # Video details (if found)
├── 6-delete-live-input.json           # Delete response (if cleaned up)
└── credentials.txt                    # Live Input UID and RTMP URL
```

### What to Look For

Review the JSON files for:

✅ **Found HLS URL** - Great! Document where it is
- `result.playback.hls`?
- `result.hls`?
- Somewhere else?

✅ **Found Video Asset** - Great! Confirms architecture
- Look for `result.created.uid`
- Check if it appears before or after streaming

❌ **No HLS URL** - Important finding!
- May need different API endpoint
- May need to query Video asset separately
- Helps plan implementation

## Using the Results

### Update Documentation

Once you know the real structure:

1. Update `zap-stream-core/notes/Cloudflare-Live-Stream-API-docs.md`
2. Replace "UNVALIDATED" sections with actual findings
3. Add any new fields discovered

### Implement Step 3A

Now you have real data to implement:

```rust
async fn get_hls_url(&self, stream_id: &str) -> Result<String> {
    // Use the ACTUAL field path discovered from testing
    // Not guesses from third-party documentation
}
```

## Troubleshooting

### "CLOUDFLARE_ACCOUNT_ID environment variable not set"

```bash
# Check if variable is set
echo $CLOUDFLARE_ACCOUNT_ID

# If empty, set it
export CLOUDFLARE_ACCOUNT_ID="your-account-id"
```

### "CLOUDFLARE_API_TOKEN environment variable not set"

```bash
# Check if variable is set  
echo $CLOUDFLARE_API_TOKEN

# If empty, set it
export CLOUDFLARE_API_TOKEN="your-token"
```

### "jq: command not found"

```bash
# macOS
brew install jq

# Ubuntu/Debian
sudo apt-get install jq

# CentOS/RHEL
sudo yum install jq
```

### API Returns 403 Forbidden

- Check your API token has correct permissions
- Ensure token includes both `Stream:Read` and `Stream:Write`
- Verify token is for the correct account

### API Returns 401 Unauthorized

- Token may be expired or invalid
- Generate a new token from Cloudflare dashboard
- Make sure you copied the complete token

## Cost

- Cloudflare Stream free tier: 1,000 minutes/month
- This test uses: < 1 minute
- **Total cost: $0**

## Next Steps

After running this script:

1. Review all JSON files in output directory
2. Document actual API structure
3. Update Cloudflare API docs with findings
4. Implement Step 3A with verified information
5. No more guessing about HLS URLs!
