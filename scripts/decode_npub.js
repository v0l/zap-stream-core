#!/usr/bin/env node
/**
 * Simple npub decoder using nostr-tools
 * Converts npub to hex public key
 */

const { nip19 } = require("nostr-tools");

function main() {
  if (process.argv.length !== 3) {
    console.error("Usage: decode_npub.js <npub>");
    process.exit(1);
  }

  const [, , npub] = process.argv;

  try {
    const decoded = nip19.decode(npub);
    if (decoded.type !== "npub") {
      throw new Error("Invalid npub format");
    }
    // Output just the hex string
    console.log(decoded.data);
  } catch (error) {
    console.error("Error:", error.message);
    process.exit(1);
  }
}

main();
