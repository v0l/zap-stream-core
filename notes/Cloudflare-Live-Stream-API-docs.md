Stream
Live Inputs
stream.live_inputs

Methods


List Live Inputs -> Envelope<{ liveInputs, range, total }>
get
/accounts/{account_id}/stream/live_inputs
Lists the live inputs created for an account. To get the credentials needed to stream to a specific live input, request a single live input.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write Stream Read

path Parameters
account_id: string
(maxLength: 32)
Identifier.

query Parameters
include_counts: booleanOptional
Includes the total number of videos associated with the submitted query parameters.

Response fields

errors: Array<{ code, message, documentation_url, 1 more... }>

messages: Array<{ code, message, documentation_url, 1 more... }>

success: true
Whether the API call was successful.


result: { liveInputs, range, total }Optional
cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN"
200
Example

{
  "errors": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "messages": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "success": true,
  "result": {
    "liveInputs": [
      {
        "created": "2014-01-02T02:20:00Z",
        "deleteRecordingAfterDays": 45,
        "meta": {
          "name": "test stream 1"
        },
        "modified": "2014-01-02T02:20:00Z",
        "uid": "66be4bf738797e01e1fca35a7bdecdcd"
      }
    ],
    "range": 1000,
    "total": 35586
  }
}

Retrieve A Live Input -> Envelope<LiveInput>
get
/accounts/{account_id}/stream/live_inputs/{live_input_identifier}
Retrieves details of an existing live input.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write Stream Read

path Parameters
account_id: string
(maxLength: 32)
Identifier.

live_input_identifier: string
(maxLength: 32)
A unique identifier for a live input.

Response fields

errors: Array<{ code, message, documentation_url, 1 more... }>

messages: Array<{ code, message, documentation_url, 1 more... }>

success: true
Whether the API call was successful.

result: LiveInputOptional
Details about a live input.

cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_IDENTIFIER \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN"
200
Example

{
  "errors": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "messages": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "success": true,
  "result": {
    "created": "2014-01-02T02:20:00Z",
    "deleteRecordingAfterDays": 45,
    "meta": {
      "name": "test stream 1"
    },
    "modified": "2014-01-02T02:20:00Z",
    "recording": {
      "allowedOrigins": [
        "example.com"
      ],
      "hideLiveViewerCount": false,
      "mode": "off",
      "requireSignedURLs": false,
      "timeoutSeconds": 0
    },
    "rtmps": {
      "streamKey": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "rtmpsPlayback": {
      "streamKey": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "srt": {
      "passphrase": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "streamId": "f256e6ea9341d51eea64c9454659e576",
      "url": "srt://live.cloudflare.com:778"
    },
    "srtPlayback": {
      "passphrase": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "streamId": "f256e6ea9341d51eea64c9454659e576",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "status": "connected",
    "uid": "66be4bf738797e01e1fca35a7bdecdcd",
    "webRTC": {
      "url": "https://customer-m033z5x00ks6nunl.cloudflarestream.com/b236bde30eb07b9d01318940e5fc3edake34a3efb3896e18f2dc277ce6cc993ad/webRTC/publish"
    },
    "webRTCPlayback": {
      "url": "https://customer-m033z5x00ks6nunl.cloudflarestream.com/b236bde30eb07b9d01318940e5fc3edake34a3efb3896e18f2dc277ce6cc993ad/webRTC/play"
    }
  }
}

Create A Live Input -> Envelope<LiveInput>
post
/accounts/{account_id}/stream/live_inputs
Creates a live input, and returns credentials that you or your users can use to stream live video to Cloudflare Stream.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write

path Parameters
account_id: string
(maxLength: 32)
Identifier.

Body parameters
defaultCreator: stringOptional
Sets the creator ID asssociated with this live input.

deleteRecordingAfterDays: numberOptional
(minimum: 30)
Indicates the number of days after which the live inputs recordings will be deleted. When a stream completes and the recording is ready, the value is used to calculate a scheduled deletion date for that recording. Omit the field to indicate no change, or include with a null value to remove an existing scheduled deletion.

meta: unknownOptional
A user modifiable key-value store used to reference other systems of record for managing live inputs.


recording: { allowedOrigins, hideLiveViewerCount, mode, 2 more... }Optional
Records the input to a Cloudflare Stream video. Behavior depends on the mode. In most cases, the video will initially be viewable as a live video and transition to on-demand after a condition is satisfied.

Response fields

errors: Array<{ code, message, documentation_url, 1 more... }>

messages: Array<{ code, message, documentation_url, 1 more... }>

success: true
Whether the API call was successful.

result: LiveInputOptional
Details about a live input.

cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs \
    -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    -d '{
          "deleteRecordingAfterDays": 45,
          "meta": {
            "name": "test stream 1"
          },
          "recording": {
            "hideLiveViewerCount": false,
            "mode": "off",
            "requireSignedURLs": false,
            "timeoutSeconds": 0
          }
        }'
200
Example

{
  "errors": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "messages": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "success": true,
  "result": {
    "created": "2014-01-02T02:20:00Z",
    "deleteRecordingAfterDays": 45,
    "meta": {
      "name": "test stream 1"
    },
    "modified": "2014-01-02T02:20:00Z",
    "recording": {
      "allowedOrigins": [
        "example.com"
      ],
      "hideLiveViewerCount": false,
      "mode": "off",
      "requireSignedURLs": false,
      "timeoutSeconds": 0
    },
    "rtmps": {
      "streamKey": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "rtmpsPlayback": {
      "streamKey": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "srt": {
      "passphrase": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "streamId": "f256e6ea9341d51eea64c9454659e576",
      "url": "srt://live.cloudflare.com:778"
    },
    "srtPlayback": {
      "passphrase": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "streamId": "f256e6ea9341d51eea64c9454659e576",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "status": "connected",
    "uid": "66be4bf738797e01e1fca35a7bdecdcd",
    "webRTC": {
      "url": "https://customer-m033z5x00ks6nunl.cloudflarestream.com/b236bde30eb07b9d01318940e5fc3edake34a3efb3896e18f2dc277ce6cc993ad/webRTC/publish"
    },
    "webRTCPlayback": {
      "url": "https://customer-m033z5x00ks6nunl.cloudflarestream.com/b236bde30eb07b9d01318940e5fc3edake34a3efb3896e18f2dc277ce6cc993ad/webRTC/play"
    }
  }
}

Update A Live Input -> Envelope<LiveInput>
put
/accounts/{account_id}/stream/live_inputs/{live_input_identifier}
Updates a specified live input.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write

path Parameters
account_id: string
(maxLength: 32)
Identifier.

live_input_identifier: string
(maxLength: 32)
A unique identifier for a live input.

Body parameters
defaultCreator: stringOptional
Sets the creator ID asssociated with this live input.

deleteRecordingAfterDays: numberOptional
(minimum: 30)
Indicates the number of days after which the live inputs recordings will be deleted. When a stream completes and the recording is ready, the value is used to calculate a scheduled deletion date for that recording. Omit the field to indicate no change, or include with a null value to remove an existing scheduled deletion.

meta: unknownOptional
A user modifiable key-value store used to reference other systems of record for managing live inputs.


recording: { allowedOrigins, hideLiveViewerCount, mode, 2 more... }Optional
Records the input to a Cloudflare Stream video. Behavior depends on the mode. In most cases, the video will initially be viewable as a live video and transition to on-demand after a condition is satisfied.

Response fields

errors: Array<{ code, message, documentation_url, 1 more... }>

messages: Array<{ code, message, documentation_url, 1 more... }>

success: true
Whether the API call was successful.

result: LiveInputOptional
Details about a live input.

cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_IDENTIFIER \
    -X PUT \
    -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    -d '{
          "deleteRecordingAfterDays": 45,
          "meta": {
            "name": "test stream 1"
          },
          "recording": {
            "hideLiveViewerCount": false,
            "mode": "off",
            "requireSignedURLs": false,
            "timeoutSeconds": 0
          }
        }'
200
Example

{
  "errors": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "messages": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "success": true,
  "result": {
    "created": "2014-01-02T02:20:00Z",
    "deleteRecordingAfterDays": 45,
    "meta": {
      "name": "test stream 1"
    },
    "modified": "2014-01-02T02:20:00Z",
    "recording": {
      "allowedOrigins": [
        "example.com"
      ],
      "hideLiveViewerCount": false,
      "mode": "off",
      "requireSignedURLs": false,
      "timeoutSeconds": 0
    },
    "rtmps": {
      "streamKey": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "rtmpsPlayback": {
      "streamKey": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "srt": {
      "passphrase": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "streamId": "f256e6ea9341d51eea64c9454659e576",
      "url": "srt://live.cloudflare.com:778"
    },
    "srtPlayback": {
      "passphrase": "2fb3cb9f17e68a2568d6ebed8d5505eak3ceaf8c9b1f395e1b76b79332497cada",
      "streamId": "f256e6ea9341d51eea64c9454659e576",
      "url": "rtmps://live.cloudflare.com:443/live/"
    },
    "status": "connected",
    "uid": "66be4bf738797e01e1fca35a7bdecdcd",
    "webRTC": {
      "url": "https://customer-m033z5x00ks6nunl.cloudflarestream.com/b236bde30eb07b9d01318940e5fc3edake34a3efb3896e18f2dc277ce6cc993ad/webRTC/publish"
    },
    "webRTCPlayback": {
      "url": "https://customer-m033z5x00ks6nunl.cloudflarestream.com/b236bde30eb07b9d01318940e5fc3edake34a3efb3896e18f2dc277ce6cc993ad/webRTC/play"
    }
  }
}

Delete A Live Input ->
delete
/accounts/{account_id}/stream/live_inputs/{live_input_identifier}
Prevents a live input from being streamed to and makes the live input inaccessible to any future API calls.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write

path Parameters
account_id: string
(maxLength: 32)
Identifier.

live_input_identifier: string
(maxLength: 32)
A unique identifier for a live input.

cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_IDENTIFIER \
    -X DELETE \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN"
Domain types


LiveInput = {
Details about a live input.

created: stringOptional
(format: date-time)
The date and time the live input was created.

deleteRecordingAfterDays: numberOptional
(minimum: 30)
Indicates the number of days after which the live inputs recordings will be deleted. When a stream completes and the recording is ready, the value is used to calculate a scheduled deletion date for that recording. Omit the field to indicate no change, or include with a null value to remove an existing scheduled deletion.

meta: unknownOptional
A user modifiable key-value store used to reference other systems of record for managing live inputs.

modified: stringOptional
(format: date-time)
The date and time the live input was last modified.


recording: { allowedOrigins, hideLiveViewerCount, mode, 2 more... }Optional
Records the input to a Cloudflare Stream video. Behavior depends on the mode. In most cases, the video will initially be viewable as a live video and transition to on-demand after a condition is satisfied.


rtmps: { streamKey, url }Optional
Details for streaming to an live input using RTMPS.


rtmpsPlayback: { streamKey, url }Optional
Details for playback from an live input using RTMPS.


srt: { passphrase, streamId, url }Optional
Details for streaming to a live input using SRT.


srtPlayback: { passphrase, streamId, url }Optional
Details for playback from an live input using SRT.


status: "connected" | "reconnected" | "reconnecting" | 5 more...OptionalNullable
The connection status of a live input.

uid: stringOptional
(maxLength: 32)
A unique identifier for a live input.


webRTC: { url }Optional
Details for streaming to a live input using WebRTC.


webRTCPlayback: { url }Optional
Details for playback from a live input using WebRTC.

}
Stream
Live Inputs
Outputs
stream.live_inputs.outputs

Methods


List All Outputs Associated With A Specified Live Input -> SinglePage<Output>
get
/accounts/{account_id}/stream/live_inputs/{live_input_identifier}/outputs
Retrieves all outputs associated with a specified live input.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write Stream Read

path Parameters
account_id: string
(maxLength: 32)
Identifier.

live_input_identifier: string
(maxLength: 32)
A unique identifier for a live input.

Response fields

errors: Array<{ code, message, documentation_url, 1 more... }>

messages: Array<{ code, message, documentation_url, 1 more... }>

success: true
Whether the API call was successful.


result: Array<Output>Optional
cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_IDENTIFIER/outputs \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN"
200
Example

{
  "errors": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "messages": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "success": true,
  "result": [
    {
      "enabled": true,
      "streamKey": "uzya-f19y-g2g9-a2ee-51j2",
      "uid": "baea4d9c515887b80289d5c33cf01145",
      "url": "rtmp://a.rtmp.youtube.com/live2"
    }
  ]
}

Create A New Output Connected To A Live Input -> Envelope<Output>
post
/accounts/{account_id}/stream/live_inputs/{live_input_identifier}/outputs
Creates a new output that can be used to simulcast or restream live video to other RTMP or SRT destinations. Outputs are always linked to a specific live input — one live input can have many outputs.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

path Parameters
account_id: string
(maxLength: 32)
Identifier.

live_input_identifier: string
(maxLength: 32)
A unique identifier for a live input.

Body parameters
streamKey: string
The streamKey used to authenticate against an output's target.

url: string
The URL an output uses to restream.

enabled: booleanOptional
(default: true)
When enabled, live video streamed to the associated live input will be sent to the output URL. When disabled, live video will not be sent to the output URL, even when streaming to the associated live input. Use this to control precisely when you start and stop simulcasting to specific destinations like YouTube and Twitch.

Response fields

errors: Array<{ code, message, documentation_url, 1 more... }>

messages: Array<{ code, message, documentation_url, 1 more... }>

success: true
Whether the API call was successful.

result: OutputOptional
cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_IDENTIFIER/outputs \
    -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    -d '{
          "streamKey": "uzya-f19y-g2g9-a2ee-51j2",
          "url": "rtmp://a.rtmp.youtube.com/live2",
          "enabled": true
        }'
200
Example

{
  "errors": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "messages": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "success": true,
  "result": {
    "enabled": true,
    "streamKey": "uzya-f19y-g2g9-a2ee-51j2",
    "uid": "baea4d9c515887b80289d5c33cf01145",
    "url": "rtmp://a.rtmp.youtube.com/live2"
  }
}

Update An Output -> Envelope<Output>
put
/accounts/{account_id}/stream/live_inputs/{live_input_identifier}/outputs/{output_identifier}
Updates the state of an output.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write

path Parameters
account_id: string
(maxLength: 32)
Identifier.

live_input_identifier: string
(maxLength: 32)
A unique identifier for a live input.

output_identifier: string
(maxLength: 32)
A unique identifier for the output.

Body parameters
enabled: boolean
(default: true)
When enabled, live video streamed to the associated live input will be sent to the output URL. When disabled, live video will not be sent to the output URL, even when streaming to the associated live input. Use this to control precisely when you start and stop simulcasting to specific destinations like YouTube and Twitch.

Response fields

errors: Array<{ code, message, documentation_url, 1 more... }>

messages: Array<{ code, message, documentation_url, 1 more... }>

success: true
Whether the API call was successful.

result: OutputOptional
cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_IDENTIFIER/outputs/$OUTPUT_IDENTIFIER \
    -X PUT \
    -H 'Content-Type: application/json' \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    -d '{
          "enabled": true
        }'
200
Example

{
  "errors": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "messages": [
    {
      "code": 1000,
      "message": "message",
      "documentation_url": "documentation_url",
      "source": {
        "pointer": "pointer"
      }
    }
  ],
  "success": true,
  "result": {
    "enabled": true,
    "streamKey": "uzya-f19y-g2g9-a2ee-51j2",
    "uid": "baea4d9c515887b80289d5c33cf01145",
    "url": "rtmp://a.rtmp.youtube.com/live2"
  }
}

Delete An Output ->
delete
/accounts/{account_id}/stream/live_inputs/{live_input_identifier}/outputs/{output_identifier}
Deletes an output and removes it from the associated live input.

Security
API Token
The preferred authorization scheme for interacting with the Cloudflare API. Create a token.

Example: Authorization: Bearer Sn3lZJTBX6kkg7OdcBUAxOO963GEIyGQqnFTOFYY

Accepted Permissions (at least one required)
Stream Write

path Parameters
account_id: string
(maxLength: 32)
Identifier.

live_input_identifier: string
(maxLength: 32)
A unique identifier for a live input.

output_identifier: string
(maxLength: 32)
A unique identifier for the output.

cURL

curl https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/stream/live_inputs/$LIVE_INPUT_IDENTIFIER/outputs/$OUTPUT_IDENTIFIER \
    -X DELETE \
    -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN"
Domain types


Output = {
enabled: booleanOptional
(default: true)
When enabled, live video streamed to the associated live input will be sent to the output URL. When disabled, live video will not be sent to the output URL, even when streaming to the associated live input. Use this to control precisely when you start and stop simulcasting to specific destinations like YouTube and Twitch.

streamKey: stringOptional
The streamKey used to authenticate against an output's target.

uid: stringOptional
(maxLength: 32)
A unique identifier for the output.

url: stringOptional
The URL an output uses to restream.

}

---

# VALIDATED: Actual API Test Results (2025-12-01)

**Source:** Direct testing with Cloudflare Stream API using real credentials

---

## ❌ Critical Finding: NO HLS URL in Live Input Response

**Testing revealed that Live Input responses DO NOT contain HLS playback URLs.**

### Actual Live Input Response Structure:

```json
{
  "result": {
    "uid": "9de18434b0ef65a93cac83ac4e76febb",
    "rtmps": {
      "url": "rtmps://live.cloudflare.com:443/live/",
      "streamKey": "3e15b77e0b7d357de912be38cd2de97dk9de18434b0ef65a93cac83ac4e76febb"
    },
    "rtmpsPlayback": {
      "url": "rtmps://live.cloudflare.com:443/live/",
      "streamKey": "6cca6c988ebab2f61559aa748baed7d6k9de18434b0ef65a93cac83ac4e76febb"
    },
    "srt": {
      "url": "srt://live.cloudflare.com:778",
      "streamId": "9de18434b0ef65a93cac83ac4e76febb",
      "passphrase": "37e0d5e5b83179d478b3f516e95be81ck9de18434b0ef65a93cac83ac4e76febb"
    },
    "srtPlayback": {
      "url": "srt://live.cloudflare.com:778",
      "streamId": "play9de18434b0ef65a93cac83ac4e76febb",
      "passphrase": "9bca869426be8e9e9cd67fb295b53a9ak9de18434b0ef65a93cac83ac4e76febb"
    },
    "webRTC": {
      "url": "https://customer-51tzzrmdygiq19h7.cloudflarestream.com/4fdfce3b81d41eedc5e0fce9d08bab39k9de18434b0ef65a93cac83ac4e76febb/webRTC/publish"
    },
    "webRTCPlayback": {
      "url": "https://customer-51tzzrmdygiq19h7.cloudflarestream.com/9de18434b0ef65a93cac83ac4e76febb/webRTC/play"
    },
    "created": "2025-12-01T01:08:50.72217Z",
    "modified": "2025-12-01T01:08:50.72217Z",
    "meta": {
      "name": "API Discovery Test - 20251201_120849"
    },
    "status": null,
    "recording": {
      "mode": "automatic",
      "timeoutSeconds": 30,
      "requireSignedURLs": false,
      "allowedOrigins": null,
      "hideLiveViewerCount": false
    },
    "deleteRecordingAfterDays": null
  },
  "success": true,
  "errors": [],
  "messages": []
}
```

### Key Findings:

1. ❌ **NO `playback.hls` field exists**
2. ❌ **NO `hls` field at any level**
3. ❌ **NO `created.uid` field** - `created` is just a timestamp string, not an object
4. ✅ **WebRTC playback URL EXISTS**: `webRTCPlayback.url`
5. ✅ **RTMPS playback URL EXISTS**: `rtmpsPlayback.url`
6. ✅ **SRT playback URL EXISTS**: `srtPlayback.url`
7. ⚠️ **Status is `null`** before streaming starts

### Implications for Implementation:

**The "unvalidated" documentation was WRONG.** There is NO:
- `created.uid` field with Video asset reference
- HLS playback URL in Live Input response
- Need to query separate Video API endpoint

**Available Playback Options:**
1. **WebRTC** (lowest latency: <1 second)
   - URL: `result.webRTCPlayback.url`
   - Best for real-time interaction
   
2. **RTMPS** (low latency: ~2-5 seconds)
   - URL: `result.rtmpsPlayback.url`
   - Stream Key: `result.rtmpsPlayback.streamKey`
   
3. **SRT** (low latency: ~2-5 seconds)
   - URL: `result.srtPlayback.url`
   - Stream ID: `result.srtPlayback.streamId`
   - Passphrase: `result.srtPlayback.passphrase`

### ✅ VALIDATED: HLS URL Architecture (Proven with Live Test)

**Test Date:** 2025-12-01  
**Test Result:** ✅ SUCCESS - Architecture confirmed working

**The Correct Architecture:**

1. **Live Input API** - Returns ingest URLs only (NO HLS)
   ```
   POST /accounts/{accountId}/stream/live_inputs
   Response: RTMP/SRT/WebRTC ingest URLs
   ```

2. **Start Streaming** - User streams to the RTMP URL

3. **Cloudflare Auto-Creates Video Asset** - Happens automatically after streaming starts

4. **Query Videos API** - Poll with liveInput filter to get the asset:
   ```bash
   GET /accounts/{accountId}/stream?liveInput={liveInputUid}
   ```

5. **Extract HLS URL** - From the Video Asset response:
   ```json
   {
     "result": [{
       "uid": "dc5a491d252654a8bcaefd5ae1d83efa",
       "playback": {
         "hls": "https://customer-*.cloudflarestream.com/{uid}/manifest/video.m3u8",
         "dash": "https://customer-*.cloudflarestream.com/{uid}/manifest/video.mpd"
       },
       "liveInput": "baafe5d4389045d7a14da579349d511e"
     }]
   }
   ```

### Test Results:

- ✅ Asset created **immediately** after streaming started (1st poll attempt)
- ✅ HLS URL accessible with HTTP 200
- ✅ Valid HLS manifest with multiple quality streams (720p, 480p, 360p)
- ✅ Manifest includes adaptive bitrate streaming
- ✅ Audio and video tracks properly configured

### Implementation for Step 3A:

```rust
async fn get_hls_url(&self, live_input_uid: &str) -> Result<String> {
    // Poll Videos API with liveInput filter
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/stream?liveInput={}",
        self.account_id, live_input_uid
    );
    
    // Retry with exponential backoff (asset created after streaming starts)
    for attempt in 0..30 {
        let response = self.http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;
        
        let json: CloudflareResponse = response.json().await?;
        
        if let Some(asset) = json.result.first() {
            return Ok(asset.playback.hls.clone());
        }
        
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    
    Err("Video asset not created after 60 seconds")
}
```

**Key Points:**
- Asset appears almost immediately after streaming starts
- Must poll `/stream?liveInput={uid}` endpoint
- Asset has `playback.hls` and `playback.dash` fields
- Asset links back via `liveInput` field
- HLS manifest supports adaptive bitrate streaming

---

## Webhook Endpoints

**From documentation above:**
- View: `GET /accounts/{account_id}/stream/webhook`
- Create: `PUT /accounts/{account_id}/stream/webhook`
- Delete: `DELETE /accounts/{account_id}/stream/webhook`

Request/response formats and event schemas not documented in API reference above.

---

**END OF VALIDATED DOCUMENTATION**
