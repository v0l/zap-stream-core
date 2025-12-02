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

## Receive Live Webhooks · Cloudflare Stream docs

description: Stream Live offers webhooks to notify your service when an Input
  connects, disconnects, or encounters an error with Stream Live.
lastUpdated: 2025-09-04T14:40:32.000Z
source_url:
  html: https://developers.cloudflare.com/stream/stream-live/webhooks/
  md: https://developers.cloudflare.com/stream/stream-live/webhooks/index.md
---

Stream Live offers webhooks to notify your service when an Input connects, disconnects, or encounters an error with Stream Live.

Stream Live Notifications

**Who is it for?**

Customers who are using [Stream](https://developers.cloudflare.com/stream/) and want to receive webhooks with the status of their videos.

**Other options / filters**

You can input Stream Live IDs to receive notifications only about those inputs. If left blank, you will receive a list for all inputs.

The following input states will fire notifications. You can toggle them on or off:

* `live_input.connected`
* `live_input.disconnected`

**Included with**

Stream subscription.

**What should you do if you receive one?**

Stream notifications are entirely customizable by the customer. Action will depend on the customizations enabled.

## Subscribe to Stream Live Webhooks

1. In the Cloudflare dashboard, go to the **Notifications** page.

   [Go to **Notifications**](https://dash.cloudflare.com/?to=/:account/notifications)

2. Select the **Destinations** tab.

3. On the **Destinations** page under **Webhooks**, select **Create**.

4. Enter the information for your webhook and select **Save and Test**.

5. To create the notification, from the **Notifications** page, select the **All Notifications** tab.

6. Next to **Notifications**, select **Add**.

7. Under the list of products, locate **Stream** and select **Select**.

8. Enter a name and optional description.

9. Under **Webhooks**, select **Add webhook** and select your newly created webhook.

10. Select **Next**.

11. By default, you will receive webhook notifications for all Live Inputs. If you only wish to receive webhooks for certain inputs, enter a comma-delimited list of Input IDs in the text field.

12. When you are done, select **Create**.

```json
{
  "name": "Live Webhook Test",
  "text": "Notification type: Stream Live Input\nInput ID: eb222fcca08eeb1ae84c981ebe8aeeb6\nEvent type: live_input.disconnected\nUpdated at: 2022-01-13T11:43:41.855717910Z",
  "data": {
    "notification_name": "Stream Live Input",
    "input_id": "eb222fcca08eeb1ae84c981ebe8aeeb6",
    "event_type": "live_input.disconnected",
    "updated_at": "2022-01-13T11:43:41.855717910Z"
  },
  "ts": 1642074233
}
```

The `event_type` property of the data object will either be `live_input.connected`, `live_input.disconnected`, or `live_input.errored`.

If there are issues detected with the input, the `event_type` will be `live_input.errored`. Additional data will be under the `live_input_errored` json key and will include a `code` with one of the values listed below.

## Error codes

* `ERR_GOP_OUT_OF_RANGE` – The input GOP size or keyframe interval is out of range.
* `ERR_UNSUPPORTED_VIDEO_CODEC` – The input video codec is unsupported for the protocol used.
* `ERR_UNSUPPORTED_AUDIO_CODEC` – The input audio codec is unsupported for the protocol used.
* `ERR_STORAGE_QUOTA_EXHAUSTED` – The account storage quota has been exceeded. Delete older content or purcahse additional storage.
* `ERR_MISSING_SUBSCRIPTION` – Unauthorized to start a live stream. Check subscription or log into Dash for details.

```json
{
  "name": "Live Webhook Test",
  "text": "Notification type: Stream Live Input\nInput ID: 2c28dd2cc444cb77578c4840b51e43a8\nEvent type: live_input.errored\nUpdated at: 2024-07-09T18:07:51.077371662Z\nError Code: ERR_GOP_OUT_OF_RANGE\nError Message: Input GOP size or keyframe interval is out of range.\nVideo Codec: \nAudio Codec: ",
  "data": {
    "notification_name": "Stream Live Input",
    "input_id": "eb222fcca08eeb1ae84c981ebe8aeeb6",
    "event_type": "live_input.errored",
    "updated_at": "2024-07-09T18:07:51.077371662Z",
    "live_input_errored": {
      "error": {
        "code": "ERR_GOP_OUT_OF_RANGE",
        "message": "Input GOP size or keyframe interval is out of range."
      },
      "video_codec": "",
      "audio_codec": ""
    }
  },
  "ts": 1720548474,
}
```

---

## Use webhooks · Cloudflare Stream docs

description: Webhooks notify your service when videos successfully finish
  processing and are ready to stream or if your video enters an error state.
lastUpdated: 2025-09-09T16:21:39.000Z
source_url:
  html: https://developers.cloudflare.com/stream/manage-video-library/using-webhooks/
  md: https://developers.cloudflare.com/stream/manage-video-library/using-webhooks/index.md
---

Webhooks notify your service when videos successfully finish processing and are ready to stream or if your video enters an error state.

## Subscribe to webhook notifications

To subscribe to receive webhook notifications on your service or modify an existing subscription, generate an API token on the **Account API tokens** page of the Cloudflare dashboard.

[Go to **Account API tokens**](https://dash.cloudflare.com/?to=/:account/api-tokens)

The webhook notification URL must include the protocol. Only `http://` or `https://` is supported.

```bash
curl -X PUT --header 'Authorization: Bearer <API_TOKEN>' \
https://api.cloudflare.com/client/v4/accounts/<ACCOUNT_ID>/stream/webhook \
--data '{"notificationUrl":"<WEBHOOK_NOTIFICATION_URL>"}'
```

```json
{
  "result": {
    "notificationUrl": "http://www.your-service-webhook-handler.com",
    "modified": "2019-01-01T01:02:21.076571Z"
    "secret": "85011ed3a913c6ad5f9cf6c5573cc0a7"
  },
  "success": true,
  "errors": [],
  "messages": []
}
```

## Notifications

When a video on your account finishes processing, you will receive a `POST` request notification with information about the video.

Note the `status` field indicates whether the video processing finished successfully.

```javascript
{
    "uid": "dd5d531a12de0c724bd1275a3b2bc9c6",
    "readyToStream": true,
    "status": {
      "state": "ready"
    },
    "meta": {},
    "created": "2019-01-01T01:00:00.474936Z",
    "modified": "2019-01-01T01:02:21.076571Z",
    // ...
  }
```

When a video is done processing and all quality levels are encoded, the `state` field returns a `ready` state. The `ready` state can be useful if picture quality is important to you, and you only want to enable video playback when the highest quality levels are available.

If higher quality renditions are still processing, videos may sometimes return the `state` field as `ready` and an additional `pctComplete` state that is not `100`. When `pctComplete` reaches `100`, all quality resolutions are available for the video.

When at least one quality level is encoded and ready to be streamed, the `readyToStream` value returns `true`.

## Error codes

If a video could not process successfully, the `state` field returns `error`, and the `errReasonCode` returns one of the values listed below.

* `ERR_NON_VIDEO` – The upload is not a video.
* `ERR_DURATION_EXCEED_CONSTRAINT` – The video duration exceeds the constraints defined in the direct creator upload.
* `ERR_FETCH_ORIGIN_ERROR` – The video failed to download from the URL.
* `ERR_MALFORMED_VIDEO` – The video is a valid file but contains corrupt data that cannot be recovered.
* `ERR_DURATION_TOO_SHORT` – The video's duration is shorter than 0.1 seconds.
* `ERR_UNKNOWN` – If Stream cannot automatically determine why the video returned an error, the `ERR_UNKNOWN` code will be used.

In addition to the `state` field, a video's `readyToStream` field must also be `true` for a video to play.

```bash
{
  "readyToStream": true,
  "status": {
    "state": "error",
    "step": "encoding",
    "pctComplete": "39",
    "errReasonCode": "ERR_MALFORMED_VIDEO",
    "errReasonText": "The video was deemed to be corrupted or malformed.",
  }
}
```

Example: POST body for successful video encoding

```json
{
 "uid": "6b9e68b07dfee8cc2d116e4c51d6a957",
 "creator": null,
 "thumbnail": "https://customer-f33zs165nr7gyfy4.cloudflarestream.com/6b9e68b07dfee8cc2d116e4c51d6a957/thumbnails/thumbnail.jpg",
 "thumbnailTimestampPct": 0,
 "readyToStream": true,
 "status": {
   "state": "ready",
   "pctComplete": "39.000000",
   "errorReasonCode": "",
   "errorReasonText": ""
 },
 "meta": {
   "filename": "small.mp4",
   "filetype": "video/mp4",
   "name": "small.mp4",
   "relativePath": "null",
   "type": "video/mp4"
 },
 "created": "2022-06-30T17:53:12.512033Z",
 "modified": "2022-06-30T17:53:21.774299Z",
 "size": 383631,
 "preview": "https://customer-f33zs165nr7gyfy4.cloudflarestream.com/6b9e68b07dfee8cc2d116e4c51d6a957/watch",
 "allowedOrigins": [],
 "requireSignedURLs": false,
 "uploaded": "2022-06-30T17:53:12.511981Z",
 "uploadExpiry": "2022-07-01T17:53:12.511973Z",
 "maxSizeBytes": null,
 "maxDurationSeconds": null,
 "duration": 5.5,
 "input": {
   "width": 560,
   "height": 320
 },
 "playback": {
   "hls": "https://customer-f33zs165nr7gyfy4.cloudflarestream.com/6b9e68b07dfee8cc2d116e4c51d6a957/manifest/video.m3u8",
   "dash": "https://customer-f33zs165nr7gyfy4.cloudflarestream.com/6b9e68b07dfee8cc2d116e4c51d6a957/manifest/video.mpd"
 },
 "watermark": null
}
```

## Verify webhook authenticity

Cloudflare Stream will sign the webhook requests sent to your notification URLs and include the signature of each request in the `Webhook-Signature` HTTP header. This allows your application to verify the webhook requests are sent by Stream.

To verify a signature, you need to retrieve your webhook signing secret. This value is returned in the API response when you create or retrieve the webhook.

To verify the signature, get the value of the `Webhook-Signature` header, which will look similar to the example below.

`Webhook-Signature: time=1230811200,sig1=60493ec9388b44585a29543bcf0de62e377d4da393246a8b1c901d0e3e672404`

### 1. Parse the signature

Retrieve the `Webhook-Signature` header from the webhook request and split the string using the `,` character.

Split each value again using the `=` character.

The value for `time` is the current [UNIX time](https://en.wikipedia.org/wiki/Unix_time) when the server sent the request. `sig1` is the signature of the request body.

At this point, you should discard requests with timestamps that are too old for your application.

### 2. Create the signature source string

Prepare the signature source string and concatenate the following strings:

* Value of the `time` field for example `1230811200`
* Character `.`
* Webhook request body (complete with newline characters, if applicable)

Every byte in the request body must remain unaltered for successful signature verification.

### 3. Create the expected signature

Compute an HMAC with the SHA256 function (HMAC-SHA256) using your webhook secret and the source string from step 2. This step depends on the programming language used by your application.

Cloudflare's signature will be encoded to hex.

### 4. Compare expected and actual signatures

Compare the signature in the request header to the expected signature. Preferably, use a constant-time comparison function to compare the signatures.

If the signatures match, you can trust that Cloudflare sent the webhook.

## Limitations

* Webhooks will only be sent after video processing is complete, and the body will indicate whether the video processing succeeded or failed.
* Only one webhook subscription is allowed per-account.

## Examples

**Golang**

Using [crypto/hmac](https://golang.org/pkg/crypto/hmac/#pkg-overview):

```go
package main


import (
 "crypto/hmac"
 "crypto/sha256"
 "encoding/hex"
 "log"
)


func main() {
 secret := []byte("secret from the Cloudflare API")
 message := []byte("string from step 2")


 hash := hmac.New(sha256.New, secret)
 hash.Write(message)


 hashToCheck := hex.EncodeToString(hash.Sum(nil))


 log.Println(hashToCheck)
}
```

**Node.js**

```js
    var crypto = require('crypto');


    var key = 'secret from the Cloudflare API';
    var message = 'string from step 2';


    var hash = crypto.createHmac('sha256', key).update(message);


    hash.digest('hex');
```

**Ruby**

```ruby
    require 'openssl'


    key = 'secret from the Cloudflare API'
    message = 'string from step 2'


    OpenSSL::HMAC.hexdigest('sha256', key, message)
```

**In JavaScript (for example, to use in Cloudflare Workers)**

```javascript
    const key = 'secret from the Cloudflare API';
    const message = 'string from step 2';


    const getUtf8Bytes = str =>
      new Uint8Array(
        [...decodeURIComponent(encodeURIComponent(str))].map(c => c.charCodeAt(0))
      );


    const keyBytes = getUtf8Bytes(key);
    const messageBytes = getUtf8Bytes(message);


    const cryptoKey = await crypto.subtle.importKey(
      'raw', keyBytes, { name: 'HMAC', hash: 'SHA-256' },
      true, ['sign']
    );
    const sig = await crypto.subtle.sign('HMAC', cryptoKey, messageBytes);


    [...new Uint8Array(sig)].map(b => b.toString(16).padStart(2, '0')).join('');
```



**END OF VALIDATED DOCUMENTATION**
