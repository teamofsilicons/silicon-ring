// Next.js collects anonymous usage data unless told not to. Ring sends no telemetry, so every Next command goes through here.
process.env.NEXT_TELEMETRY_DISABLED = '1';
await import('next/dist/bin/next');
