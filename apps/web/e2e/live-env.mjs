// Where the live test server listens and keeps its data (shared by live-server.mjs and the tests).
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Stay below Linux's default ephemeral port range so outbound connections do not take it.
export const PORT = Number(process.env.LIVE_PORT ?? 27412);
const here = dirname(fileURLToPath(import.meta.url));
export const DATA = join(here, ".live-data");
export const GALLERY_PORT = Number(process.env.ADMIN_GALLERY_PORT ?? 27413);
export const GALLERY_DATA = resolve(here, "../../../target/admin-gallery-data");
