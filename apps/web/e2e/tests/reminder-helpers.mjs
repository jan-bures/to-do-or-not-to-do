import { open } from "./helpers.mjs";

export const REMINDER_TASK = "Prepare the November board update";
export const REMINDER_PATH = "/app?fixture=reminder-alerts";

export async function mockNotifications(page, permission, { noChannel = false, cannotConstruct = false } = {}) {
  await page.addInitScript(({ permission, noChannel, cannotConstruct }) => {
    const mock = window.reminderMock = { permission, requests: [], notifications: [], focused: 0 };
    window.focus = () => { mock.focused += 1; };
    if (noChannel) window.BroadcastChannel = undefined;
    window.Notification = permission === "unsupported" ? undefined : class {
      static get permission() { return mock.permission; }
      static async requestPermission() {
        mock.requests.push(navigator.userActivation.isActive);
        return mock.permission = "granted";
      }
      constructor(title, options) {
        if (cannotConstruct) throw new TypeError("Constructing a notification is not supported");
        this.title = title;
        this.options = options;
        this.closed = false;
        mock.notifications.push(this);
      }
      close() { this.closed = true; }
    };
  }, { permission, noChannel, cannotConstruct });
}

export async function beforeReminder(page, path = REMINDER_PATH) {
  // The fixture reminder is at 14:30 UTC, or 15:30 in the fixture browser zone.
  await page.clock.install({ time: new Date("2026-11-21T14:29:45Z") });
  await page.clock.pauseAt(new Date("2026-11-21T14:29:50Z"));
  await open(page, path);
  await page.clock.runFor(20);
}

export const reminderToast = (page) => page.locator(".toast-reminder");