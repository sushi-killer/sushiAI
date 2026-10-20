"use strict";

/**
 * The life of the saved Vault token: it is looked up (lookup-self), renewed
 * while it can be (renew-self) and the owner is told before it dies. Only the
 * non-secret metadata is kept (ttl, renewable, expiry, max ttl, period); the
 * token and any reply body are never stored or logged. One timer drives it.
 */
const TIMEOUT_MS = 5000;
const MIN_RENEW_DELAY_MS = 60 * 1000;
const WARN_BEFORE_MS = 24 * 60 * 60 * 1000;
const RETRY_BASE_MS = 30 * 1000;
const RETRY_MAX_MS = 30 * 60 * 1000;
// setTimeout wraps around above 2^31 - 1 ms; a longer wait is split in two.
const MAX_TIMER_MS = 2 ** 31 - 1;
const TOKEN_NOTICE = "vault:token";
const NEW_TOKEN = "save a new one in Settings → Vault";

/** One request to Vault with the token. A redirect is not followed: the token
 * would go to wherever it points. Rejects on a network error or a timeout. */
function vaultFetch(token, url, method = "GET") {
  return fetch(url, {
    method,
    headers: { "X-Vault-Token": token },
    redirect: "manual",
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
}

const isCount = (value) => Number.isFinite(value) && value >= 0;

/** The non-secret part of a lookup-self reply, or null when it is not one.
 * `ttl` is the seconds left; `expire_time` is null for a token that never
 * expires; `creation_time` is epoch seconds. */
function metadataOf(body, now) {
  const data = body?.data;
  if (!data || typeof data !== "object" || !isCount(data.ttl)) return null;
  const expireTime = Date.parse(data.expire_time ?? "");
  let expiresAt = null;
  if (data.ttl > 0) expiresAt = now + data.ttl * 1000;
  else if (Number.isFinite(expireTime)) expiresAt = expireTime;
  const maxTtl = isCount(data.explicit_max_ttl) ? data.explicit_max_ttl : 0;
  const created = isCount(data.creation_time) ? data.creation_time : 0;
  return {
    expiresAt,
    renewable: data.renewable === true,
    period: isCount(data.period) ? data.period : 0,
    // The last moment a renewal can reach; null when there is no max ttl.
    maxEnd: maxTtl && created ? (created + maxTtl) * 1000 : null,
  };
}

/** A renewal moves the end of this token further out. */
const canRenew = (meta) =>
  meta.expiresAt !== null &&
  meta.renewable &&
  (meta.period > 0 || meta.maxEnd === null || meta.maxEnd > meta.expiresAt);

const when = (ms) =>
  `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`;

/**
 * @param {{
 *   loadToken: () => { token: string | null, unreadable: boolean },
 *   getAddress: () => string,
 *   isEnabled: () => boolean,
 *   notices: { publish: Function, retract: Function },
 *   source: string,
 *   now?: () => number,
 *   setTimer?: (callback: () => void, ms: number) => unknown,
 *   clearTimer?: (handle: unknown) => void,
 * }} deps
 */
function createTokenWatch({
  loadToken,
  getAddress,
  isEnabled,
  notices,
  source,
  now = Date.now,
  setTimer = setTimeout,
  clearTimer = clearTimeout,
}) {
  // Bumped by every new check and by stop, so an older reply is dropped.
  let epoch = 0;
  let timer = null;
  let retries = 0;
  let meta = null;
  let state = "unchecked";
  let noticeText = null;

  const stopTimer = () => {
    if (timer !== null) clearTimer(timer);
    timer = null;
  };

  function say(text, kind) {
    if (text === noticeText) return;
    try {
      if (text === null) notices.retract(source, TOKEN_NOTICE);
      else
        notices.publish(source, {
          key: TOKEN_NOTICE,
          kind,
          title: "Vault",
          header: "Vault token",
          body: text,
        });
      noticeText = text;
    } catch {
      // A notice that cannot be shown never stops the token.
    }
  }

  function schedule(at, renew) {
    stopTimer();
    const wait = Math.max(0, at - now());
    const tooLong = wait > MAX_TIMER_MS;
    // The callback returns its work, so a test clock can wait for it.
    const handle = setTimer(
      () =>
        run(renew && !tooLong).catch(() => {
          console.warn("Vault: token check failed");
        }),
      Math.min(wait, MAX_TIMER_MS),
    );
    handle?.unref?.();
    timer = handle;
  }

  function retryLater(renew) {
    const wait = Math.min(RETRY_BASE_MS * 2 ** retries, RETRY_MAX_MS);
    retries += 1;
    state = "unreachable";
    schedule(now() + wait, renew);
  }

  function dead() {
    state = "expired";
    say(`Vault token expired or was rejected (403); ${NEW_TOKEN}.`, "failed");
  }

  /** What a request to Vault came to: ok (with the parsed body when asked),
   * dead (the token is refused) or retry (network, timeout, bad reply). */
  async function ask(token, address, method, path, wantBody) {
    let response;
    try {
      response = await vaultFetch(token, `${address}/v1/${path}`, method);
    } catch {
      return { kind: "retry" };
    }
    if (response.status === 401 || response.status === 403) {
      response.body?.cancel().catch(() => {});
      return { kind: "dead" };
    }
    if (!response.ok) {
      response.body?.cancel().catch(() => {});
      return { kind: "retry" };
    }
    if (!wantBody) {
      response.body?.cancel().catch(() => {});
      return { kind: "ok" };
    }
    try {
      return { kind: "ok", body: await response.json() };
    } catch {
      return { kind: "retry" };
    }
  }

  /** Looks the token up, renewing it first when asked, and plans the next
   * step: a renewal at about half of the time left, or a notice before a
   * token that cannot be renewed ends. */
  async function run(renew) {
    const mine = ++epoch;
    stopTimer();
    const { token, unreadable } = loadToken();
    if (unreadable) {
      meta = null;
      say(
        `Vault token cannot be read from secure storage; ${NEW_TOKEN}.`,
        "failed",
      );
      return;
    }
    if (!token) {
      meta = null;
      say(null);
      return;
    }
    const address = getAddress();
    if (!address || !isEnabled()) {
      state = "unchecked";
      return;
    }
    if (renew) {
      const renewed = await ask(
        token,
        address,
        "POST",
        "auth/token/renew-self",
        false,
      );
      if (mine !== epoch) return;
      if (renewed.kind === "dead") return dead();
      if (renewed.kind === "retry") return retryLater(true);
    }
    const found = await ask(
      token,
      address,
      "GET",
      "auth/token/lookup-self",
      true,
    );
    if (mine !== epoch) return;
    if (found.kind === "dead") return dead();
    const next = found.kind === "ok" ? metadataOf(found.body, now()) : null;
    if (!next) return retryLater(false);
    retries = 0;
    meta = next;
    state = "valid";
    if (meta.expiresAt === null) return say(null);
    const left = meta.expiresAt - now();
    if (canRenew(meta)) {
      say(null);
      schedule(now() + Math.max(left / 2, MIN_RENEW_DELAY_MS), true);
      return;
    }
    // Nothing can extend this token: warn a day ahead, then check at its end.
    if (left <= WARN_BEFORE_MS) {
      say(`Vault token expires ${when(meta.expiresAt)}; ${NEW_TOKEN}.`, "info");
      schedule(meta.expiresAt, false);
    } else schedule(meta.expiresAt - WARN_BEFORE_MS, false);
  }

  return {
    /** Looks the saved token up now and plans what follows. */
    check: () => run(false),
    /** The token is gone: forget its state and stop the timer. */
    clear() {
      epoch += 1;
      stopTimer();
      meta = null;
      retries = 0;
      state = "unchecked";
      say(null);
    },
    /** Stops the timer for good, for example at quit. */
    stop() {
      epoch += 1;
      stopTimer();
    },
    status() {
      const { token, unreadable } = loadToken();
      const shown = unreadable ? "unreadable" : token ? state : "none";
      return {
        saved: shown !== "none",
        state: shown,
        expiresAt: meta?.expiresAt ?? null,
        autoRenews: Boolean(meta && canRenew(meta)),
      };
    },
  };
}

module.exports = {
  createTokenWatch,
  vaultFetch,
  TIMEOUT_MS,
  MIN_RENEW_DELAY_MS,
  WARN_BEFORE_MS,
};
