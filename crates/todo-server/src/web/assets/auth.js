// Passkey ceremonies for the sign-in, setup and admin pages. Served by the server itself; no
// dependencies. The server sends WebAuthn options as JSON with base64url-encoded binary fields; this
// file converts them for navigator.credentials and encodes the result back.
"use strict";

(function () {
  function b64urlToBuf(s) {
    const pad = "=".repeat((4 - (s.length % 4)) % 4);
    const bin = atob((s + pad).replace(/-/g, "+").replace(/_/g, "/"));
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out.buffer;
  }

  function bufToB64url(buf) {
    const bytes = new Uint8Array(buf);
    let bin = "";
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }

  function creationOptions(o) {
    const pk = o.publicKey;
    pk.challenge = b64urlToBuf(pk.challenge);
    pk.user.id = b64urlToBuf(pk.user.id);
    (pk.excludeCredentials || []).forEach(function (c) { c.id = b64urlToBuf(c.id); });
    return o;
  }

  function requestOptions(o) {
    const pk = o.publicKey;
    pk.challenge = b64urlToBuf(pk.challenge);
    (pk.allowCredentials || []).forEach(function (c) { c.id = b64urlToBuf(c.id); });
    delete o.mediation;
    return o;
  }

  function encodeRegistration(c) {
    const r = c.response;
    return {
      id: c.id,
      rawId: bufToB64url(c.rawId),
      type: c.type,
      response: {
        attestationObject: bufToB64url(r.attestationObject),
        clientDataJSON: bufToB64url(r.clientDataJSON),
        transports: typeof r.getTransports === "function" ? r.getTransports() : undefined,
      },
      extensions: c.getClientExtensionResults(),
    };
  }

  function encodeAssertion(c) {
    const r = c.response;
    return {
      id: c.id,
      rawId: bufToB64url(c.rawId),
      type: c.type,
      response: {
        authenticatorData: bufToB64url(r.authenticatorData),
        clientDataJSON: bufToB64url(r.clientDataJSON),
        signature: bufToB64url(r.signature),
        userHandle: r.userHandle ? bufToB64url(r.userHandle) : null,
      },
      extensions: c.getClientExtensionResults(),
    };
  }

  function csrfToken() {
    const m = document.querySelector('meta[name="csrf-token"]');
    return m ? m.getAttribute("content") : null;
  }

  async function post(url, body) {
    const headers = { "Content-Type": "application/json" };
    const csrf = csrfToken();
    if (csrf) headers["X-CSRF-Token"] = csrf;
    const res = await fetch(url, {
      method: "POST",
      headers: headers,
      credentials: "same-origin",
      body: JSON.stringify(body || {}),
    });
    let data = null;
    try { data = await res.json(); } catch (_) { data = null; }
    if (!res.ok) {
      const msg = data && data.error ? data.error.message : "request failed (" + res.status + ")";
      throw new Error(msg);
    }
    return data;
  }

  function status(msg, isError) {
    const el = document.getElementById("status");
    if (!el) return;
    el.textContent = msg;
    el.className = isError ? "error" : "";
  }

  function supported() {
    if (!window.PublicKeyCredential || !navigator.credentials) {
      status("This browser does not support passkeys.", true);
      return false;
    }
    return true;
  }

  async function register(prefix, startBody, extra) {
    const s = await post(prefix + "/start", startBody);
    const cred = await navigator.credentials.create(creationOptions(s.options));
    const body = Object.assign({ ceremony: s.ceremony, credential: encodeRegistration(cred) }, extra || {});
    return post(prefix + "/finish", body);
  }

  async function assert(prefix, extra) {
    const s = await post(prefix + "/start", {});
    const cred = await navigator.credentials.get(requestOptions(s.options));
    const body = Object.assign({ ceremony: s.ceremony, credential: encodeAssertion(cred) }, extra || {});
    return post(prefix + "/finish", body);
  }

  function guarded(fn) {
    return async function (ev) {
      if (ev) ev.preventDefault();
      if (!supported()) return;
      const el = ev && ev.currentTarget;
      if (el && "disabled" in el) el.disabled = true;
      try {
        await fn(el);
      } catch (e) {
        status(e && e.message ? e.message : String(e), true);
      } finally {
        if (el && "disabled" in el) el.disabled = false;
      }
    };
  }

  // True once a form result replaced the page in place: the address bar still shows the page the
  // form was on, so "go back" after a step-up means reloading it, not leaving it.
  let swapped = false;

  const actions = {
    login: guarded(async function (el) {
      status("Waiting for your passkey…");
      const r = await assert("/auth/login", { next: el.getAttribute("data-next") || "/admin" });
      window.location.assign(r.redirect || "/admin");
    }),
    "step-up": guarded(async function (el) {
      status("Waiting for your passkey…");
      await assert("/auth/step-up");
      const next = el.getAttribute("data-next");
      if (next) window.location.replace(next);
      else if (el.getAttribute("data-after") === "back" && !swapped) window.history.back();
      else window.location.reload();
    }),
    "add-passkey": guarded(async function () {
      const name = document.getElementById("passkey-name");
      status("Waiting for your new passkey…");
      await register("/auth/passkeys", {}, { name: name ? name.value : "" });
      window.location.reload();
    }),
  };

  function bindInvite(form) {
    let secret = window.location.hash.slice(1);
    // Clear the address bar and this history entry before user interaction or navigation.
    if (window.location.hash) window.history.replaceState(null, "", "/pair/invite");
    const button = form.querySelector('button[type="submit"]');
    if (!/^[A-Za-z0-9_-]{43}$/.test(secret)) {
      status("Open a new invite link from your signed-in app.", true);
      button.disabled = true;
      return;
    }
    form.addEventListener("submit", guarded(async function () {
      if (button.disabled || !secret) return;
      button.disabled = true;
      try {
        const name = document.getElementById("invite-device-name").value.trim();
        status("Waiting for your new passkey…");
        const r = await register("/auth/invite", { secret: secret, device_name: name });
        secret = "";
        window.location.replace(r.redirect || "/");
      } finally {
        button.disabled = !secret;
      }
    }));
  }

  const setup = guarded(async function (form) {
    const code = form.querySelector('input[name="code"]').value;
    status("Waiting for your passkey…");
    const r = await register("/auth/setup", { code: code });
    window.location.assign(r.redirect || "/admin");
  });

  // Admin forms post in place: the result replaces the page body without a navigation, so a form
  // that redirects back to the page it was on (add a key, remove a sender…) keeps the scroll
  // position. Any other result (a one-time token, an error) shows from the top. Sign-out and setup
  // navigate as usual. Without JavaScript the forms still work as plain posts.
  async function submitInPlace(ev) {
    const form = ev.currentTarget;
    ev.preventDefault();
    const submitter = ev.submitter || form.querySelector('button[type="submit"]');
    if (submitter) submitter.disabled = true;
    let res;
    try {
      res = await fetch(form.action, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams(new FormData(form)),
      });
    } catch (e) {
      if (submitter) submitter.disabled = false;
      status("Could not reach the server: " + (e && e.message ? e.message : String(e)), true);
      return;
    }
    const doc = new DOMParser().parseFromString(await res.text(), "text/html");
    if (!doc.body || !doc.querySelector("main")) {
      // Not one of our pages: show whatever it was.
      window.location.assign(res.url);
      return;
    }
    const back = res.redirected && new URL(res.url).pathname === window.location.pathname;
    const y = window.scrollY;
    document.title = doc.title;
    document.body.replaceChildren(...Array.from(doc.body.childNodes).map(function (n) {
      return document.adoptNode(n);
    }));
    if (res.redirected && !back) {
      // A redirect to another page: show its address so a reload stays there.
      window.history.pushState(null, "", res.url);
    } else if (!back) {
      swapped = true;
    }
    window.scrollTo(0, back ? y : 0);
    bind(document);
  }

  function bind(root) {
    root.querySelectorAll("button[data-action]").forEach(function (b) {
      const fn = actions[b.getAttribute("data-action")];
      if (fn) b.addEventListener("click", fn);
    });
    const setupForm = root.querySelector('form[data-action="setup"]');
    if (setupForm) setupForm.addEventListener("submit", setup);
    const inviteForm = root.querySelector('form[data-action="redeem-invite"]');
    if (inviteForm) bindInvite(inviteForm);
    root.querySelectorAll('form[method="post"]:not([data-action="redeem-invite"])').forEach(function (f) {
      const action = f.getAttribute("action") || "";
      if (action.indexOf("/admin/") === 0 || action.indexOf("/pair/") === 0) {
        f.addEventListener("submit", submitInPlace);
      }
    });
  }

  document.addEventListener("DOMContentLoaded", function () { bind(document); });
  // Back and forward over an address pushed by an in-place post: load that page for real.
  window.addEventListener("popstate", function () { window.location.reload(); });
})();
