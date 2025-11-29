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
