# Optional Cloudflare Live Stream Back End

This software includes an internal RTMP ingest server backend, but it can be configured to use  Cloudflare Live Stream ingest as the backend if the host chooses.

## Cloudflare set up

You will need a Cloudflare account with access to Live Streaming

1. Configure compose-config.yaml
2. Set up webhook notifications
3. Set endpoint app configuration (recommended)
4. Set custom ingest domain (optional)

## Configure compose-config.yaml

First, in the compose-config.yaml specify your

- Backend choice (Cloudflare)
- API token
- Account ID

As follows:

```yaml
overseer:
  # Streaming backend type (options: "rml_rtmp" or "cloudflare", defaults to "rml_rtmp")
  backend: "cloudflare"
  # If cloudflare is selected enter api_token and account_id
  cloudflare:
    api-token: "my-token"
    account-id: "my-account-id"
```

Start the Docker with `docker compose up`

Your logs should show an attempted connection to your Cloudflare webhook URL
`Setting up Cloudflare webhook at: https://your.domain.name/webhooks/cloudflare⁠`

And then success if Cloudflare is able to reach it
`Webhook configured successfully, secret received`

## Set up webhook notifications

Next, set up Cloudflare to notify your webhook URL on live stream start, end and error

View Cloudflare docs at `https://developers.cloudflare.com/notifications/`

In your Cloudflare dashboard > Manage Account > Notifications, look for "All Notification" and "Destinations"

Configure your new Webhook as a destination. Press Destinations > Create and enter:

- Name (whatever you choose)
- URL (the URL from the log earlier, e.g. `https://your.domain.name/webhooks/cloudflare`)

Press Save and Test. If your Docker is running, it should show a test webhook has been received successfully.

`Received webhook test message - webhook configuration successful!`

Your Cloudflare backend is now operational and connected to Cloudflare.

## Set endpoint app configuration (recommended)

By default Zap Stream Core sets up multiple app endpoints with different capabilities and different costs. Cloudflare stream does not support this behaviour, as all streams have the same capabilities.

It is recommended to configure your Zap Stream Core endpoints to have a single endpoint that matches the capabilities of Cloudflare by editing the endpoints in the Zap Stream Admin.

- Get the Zap Stream Admin from `https://github.com/v0l/zap-stream-admin`
- Log in with your authorised user from the `compose-config.yaml`
- Visit `/ingest-endpoints` – by default "Good" and "Basic" endpoints are available

Recommended

- Remove one endpoint
- Set the other endpint to include capabilities `variant:720:30` and `dvr:720`
- Set the name and cost to suit your needs

## Set custom ingest domain (optional)

Cloudflare allows accounts to specify custom domain names for ingest.

View Cloudflare docs at `https://developers.cloudflare.com/stream/stream-live/custom-domains/`

In your Cloudflare dashboard > Media > Stream > Live inputs, look for "Custom Input Domains"

1. In your DNS registry, add the Cloudflare CNAME record to your domain
2. In your Cloudflare dashboard, add the domain into the field provided, and press "Add domain". 
DNS will take a few hours to propogate and the change and show "Active" when ready.
3. In your Zap Stream Core compose-config.yaml set the `endpoints_public_hostname` to your custom domain, e.g. `endpoints_public_hostname: "your.domain.name"`

If this is correctly configured

- Queries to your Zap Stream Core API at `/api/v1/accounts` will return your RTMP ingest endpoint with your custom domain name.
