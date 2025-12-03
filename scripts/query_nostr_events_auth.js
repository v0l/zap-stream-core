#!/usr/bin/env node

/**
 * Query Nostr Events from SW2 Relay with NIP-42 Authentication
 *
 * Usage: node query_nostr_events_auth.js [kind] [--since TIMESTAMP]
 */

const WebSocket = require("ws");
const {
  generateSecretKey,
  getPublicKey,
  finalizeEvent,
} = require("nostr-tools/pure");
const { SimplePool } = require("nostr-tools/pool");

// Configuration
const RELAY_URL = "ws://localhost:3334";
const DEFAULT_KIND = 30311;

// Generate a temporary keypair for auth
const sk = generateSecretKey();
const pk = getPublicKey(sk);

console.log(`Using pubkey for auth: ${pk}\n`);

async function queryWithAuth(kind, since) {
  const ws = new WebSocket(RELAY_URL);
  const events = [];
  let authChallenge = null;

  ws.on("open", () => {
    console.log("✓ Connected to relay");
  });

  ws.on("message", (data) => {
    const msg = JSON.parse(data.toString());
    const [type, ...rest] = msg;

    if (type === "AUTH") {
      authChallenge = rest[0];
      console.log(`✓ Received AUTH challenge: ${authChallenge}`);

      // Create NIP-42 auth event
      const authEvent = finalizeEvent(
        {
          kind: 22242,
          created_at: Math.floor(Date.now() / 1000),
          tags: [
            ["relay", RELAY_URL],
            ["challenge", authChallenge],
          ],
          content: "",
        },
        sk
      );

      console.log("✓ Sending AUTH event...");
      ws.send(JSON.stringify(["AUTH", authEvent]));

      // Now send the actual REQ
      setTimeout(() => {
        const filter = { kinds: [kind] };
        if (since) filter.since = since;
        console.log(`✓ Sending REQ for kind ${kind}...`);
        ws.send(JSON.stringify(["REQ", "query1", filter]));
      }, 100);
    } else if (type === "EVENT") {
      const [subId, event] = rest;
      events.push(event);
      console.log(`\n${"─".repeat(80)}`);
      console.log(`✓ Event ${events.length} found!`);
      console.log("─".repeat(80));
      console.log(`ID: ${event.id}`);
      console.log(`Kind: ${event.kind}`);
      console.log(`Author: ${event.pubkey}`);
      console.log(
        `Created: ${new Date(event.created_at * 1000).toISOString()}`
      );
      console.log(`\nFull JSON:`);
      console.log(JSON.stringify(event, null, 2));
    } else if (type === "EOSE") {
      console.log(`\n${"=".repeat(80)}`);
      console.log(`✓ EOSE - Found ${events.length} events of kind ${kind}`);
      console.log("=".repeat(80));
      ws.close();
    } else if (type === "CLOSED") {
      console.log(`\n✗ Subscription closed: ${rest[1]}`);
      ws.close();
    } else if (type === "OK") {
      console.log(`✓ AUTH accepted by relay`);
    }
  });

  ws.on("error", (err) => {
    console.error("✗ WebSocket error:", err.message);
    process.exit(1);
  });

  ws.on("close", () => {
    console.log("\n✓ Connection closed");
    process.exit(0);
  });

  // Timeout after 10 seconds
  setTimeout(() => {
    console.log("\n✗ Timeout");
    ws.close();
  }, 10000);
}

// Parse arguments
const args = process.argv.slice(2);
let kind = DEFAULT_KIND;
let since = null;

for (let i = 0; i < args.length; i++) {
  if (args[i] === "--since" && i + 1 < args.length) {
    since = parseInt(args[i + 1]);
    i++;
  } else if (!isNaN(parseInt(args[i]))) {
    kind = parseInt(args[i]);
  }
}

console.log(
  `Querying kind ${kind} events from SW2 relay with authentication...\n`
);
queryWithAuth(kind, since);
