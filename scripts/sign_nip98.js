#!/usr/bin/env node
/**
 * NIP-98 event signer using nostr-tools
 * Creates a signed Nostr event (kind 27235) for HTTP authentication
 */

const {
  nip19,
  getPublicKey,
  finalizeEvent,
  getEventHash,
} = require("nostr-tools");

function main() {
  if (process.argv.length !== 5) {
    console.error("Usage: sign_nip98.js <nsec> <url> <method>");
    process.exit(1);
  }

  const [, , nsec, url, method] = process.argv;

  try {
    // Decode nsec to get private key bytes
    const decoded = nip19.decode(nsec);
    if (decoded.type !== "nsec") {
      throw new Error("Invalid nsec format");
    }
    const privateKeyHex = decoded.data;

    // Derive public key
    const publicKeyHex = getPublicKey(privateKeyHex);

    // Create NIP-98 event template
    const eventTemplate = {
      kind: 27235,
      created_at: Math.floor(Date.now() / 1000),
      tags: [
        ["u", url],
        ["method", method],
      ],
      content: "",
    };

    // Sign the event (this adds id, pubkey, and sig)
    const signedEvent = finalizeEvent(eventTemplate, privateKeyHex);

    // Output as JSON
    console.log(JSON.stringify(signedEvent));
  } catch (error) {
    console.error("Error:", error.message);
    process.exit(1);
  }
}

main();
