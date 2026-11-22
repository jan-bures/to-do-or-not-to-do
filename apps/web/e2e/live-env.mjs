// Where the live test server listens and keeps its data (shared by live-server.mjs and the tests).
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Stay below Linux's default ephemeral port range so outbound connections do not take it.
export const PORT = Number(process.env.LIVE_PORT ?? 27412);
export const DATA = join(dirname(fileURLToPath(import.meta.url)), ".live-data");
