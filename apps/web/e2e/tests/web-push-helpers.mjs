import { mockNotifications } from "./reminder-helpers.mjs";

export async function mockPush(page, permission, initial = false) {
  await mockNotifications(page, permission);
  const publicKey = `B${"A".repeat(86)}`;
  const endpoint = "https://push.example.com/subscription/fixture";
  await page.addInitScript(({ initial, publicKey, endpoint }) => {
    window.pushMock = { activation: [], unsubscribed: false };
    const key = Uint8Array.from(atob(publicKey), c => c.charCodeAt(0)).buffer;
    const create = () => ({
      endpoint,
      options: { applicationServerKey: key },
      toJSON: () => ({ endpoint, expirationTime: null, keys: { p256dh: "fixture", auth: "fixture" } }),
      unsubscribe: async () => {
        window.pushMock.unsubscribed = true;
        subscription = null;
        return true;
      },
    });
    let subscription = initial ? create() : null;
    const registration = {
      active: true,
      pushManager: {
        getSubscription: async () => subscription,
        subscribe: async ({ userVisibleOnly }) => {
          window.pushMock.activation.push([navigator.userActivation.isActive, userVisibleOnly]);
          await Notification.requestPermission();
          return (subscription = create());
        },
      },
    };
    // Real service-worker installation and delivery are exercised separately.
    Object.defineProperty(navigator, "serviceWorker", { value: {
      register: async () => registration,
      getRegistration: async () => registration,
    } });
  }, { initial, publicKey, endpoint });
  const calls = [];
  let active = initial;
  await page.route(/\/api\/v1\/web-push(?:\/(?:status|test))?$/, async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    calls.push({ method, path, body: request.postDataJSON(), csrf: request.headers()["x-csrf-token"] });
    if (method === "GET") return route.fulfill({ json: { public_key: publicKey } });
    if (path.endsWith("/status")) return route.fulfill({ json: { active } });
    if (method === "PUT") active = true;
    if (method === "DELETE") active = false;
    return route.fulfill({ status: 204 });
  });
  return { calls, endpoint };
}
