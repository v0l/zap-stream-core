# Stream Index Page Implementation

This document explains the changes made to implement the enhanced index.html landing page with active streams and HLS.js player.

## Changes Made

### 1. Updated Dependencies
- Added `mustache = "0.9.0"` to `crates/zap-stream/Cargo.toml` for templating support

### 2. Enhanced index.html Template
The `crates/zap-stream/index.html` file was converted from static HTML to a Mustache template with:

#### Features:
- **Dynamic Active Streams List**: Shows live streams with clickable links
- **HLS.js Integration**: Embedded video player for streaming content
- **Responsive Design**: Clean interface with proper styling
- **Interactive Controls**: Play buttons and custom URL input

#### Template Variables:
- `{{public_url}}`: The server's public URL
- `{{#has_streams}}...{{/has_streams}}`: Conditional section for when streams exist
- `{{^has_streams}}...{{/has_streams}}`: Section shown when no streams are active
- `{{#streams}}...{{/streams}}`: Loop over active streams
- `{{title}}`: Stream title (fallback to shortened ID)
- `{{summary}}`: Optional stream description
- `{{live_url}}`: Direct link to stream's live.m3u8
- `{{viewer_count}}`: Current viewer count (if > 0)

### 3. Updated HTTP Server (http.rs)
Modified `HttpServer` to:
- Use template rendering instead of static string replacement
- Fetch active streams from database
- Build template data with stream information
- Render using mustache templating engine

Key method: `render_index()` - Fetches active streams and renders the template

### 4. Enhanced API (api.rs)
Added new methods:
- `get_active_streams()`: Retrieves live streams from database
- `get_public_url()`: Returns the configured public URL

### 5. Updated Main (main.rs)
Changed to pass the template string instead of pre-rendered HTML to HttpServer

## Usage

### For Viewers
1. Visit the server's root URL (`/` or `/index.html`)
2. See list of active streams with titles and viewer counts
3. Click "Play" button next to any stream to start playback
4. Or enter a custom stream URL in the player section

### Stream URLs
Active streams are accessible at: `/{stream_id}/live.m3u8`

### Player Features
- **Auto-detection**: Uses HLS.js for browsers that support it
- **Fallback**: Native HLS support for Safari/iOS
- **Custom URLs**: Input field for manual stream URL entry
- **Error Handling**: Alerts if HLS is not supported

## Example Output

When streams are active:
```html
<h1>Welcome to https://zap.stream</h1>
<div class="stream-list">
    <div class="stream-item">
        <div class="stream-title">Bitcoin Talk Show</div>
        <div class="stream-summary">Live discussion about Bitcoin</div>
        <div>
            <a href="/stream-abc123/live.m3u8">📺 /stream-abc123/live.m3u8</a>
            <span>👥 42 viewers</span>
        </div>
        <button onclick="playStream('/stream-abc123/live.m3u8')">Play</button>
    </div>
</div>
```

When no streams are active:
```html
<h1>Welcome to https://zap.stream</h1>
<div class="no-streams">No active streams</div>
```

## Security Considerations
- Mustache templates automatically escape HTML to prevent XSS
- Stream data is sourced from database (trusted source)
- HLS.js loaded from CDN with integrity checking

## Dependencies
- **mustache**: Template rendering
- **serde_json**: Data structure handling for templates
- **HLS.js**: Client-side HLS streaming (loaded via CDN)