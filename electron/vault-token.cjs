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
// A token whose expiry cannot be read is looked at again this often.
const LIMITED_RECHECK_MS = 6 * 60 * 60 * 1000;
// While the module is off nothing is asked; the switch is noticed this late.
const IDLE_RECHECK_MS = 30 * 60 * 1000;
// A renewal starts this long before the end, whatever the half says.
const RENEW_MARGIN_MS = 10 * 1000;
// A renewal that moves the end by less than this did not extend the token.
const MOVE_TOLERANCE_MS = 2 * 1000;
// A timer this late means the machine slept: look before renewing blindly.
const LATE_MS = 2 * 60 * 1000;
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
  (meta.maxEnd === null || meta.maxEnd > meta.expiresAt);

const when = (ms) =>
  `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`;

/**
 * @param {{
 *   loadToken: () => { token: string | null, unreadable: boolean, unavailable?: boolean },
 *   isTokenAccepted?: (token: string, address: string) => Promise<boolean | null>,
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
  isTokenAccepted = async () => null,
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
  let stopped = false;
  // Set for the saved token when Vault will not renew it, or a renewal does
  // not move its end (a system or mount max ttl); cleared with the token.
  let cannotRenew = false;

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

  const guarded = (work) =>
    work.catch(() => {
      console.warn("Vault: token check failed");
    });

  function schedule(at, renew) {
    stopTimer();
    if (stopped) return;
    const wait = Math.max(0, at - now());
    const tooLong = wait > MAX_TIMER_MS;
    // The callback returns its work, so a test clock can wait for it.
    const handle = setTimer(
      () => guarded(run(renew && !tooLong && now() - at < LATE_MS)),
      Math.min(wait, MAX_TIMER_MS),
    );
    handle?.unref?.();
    timer = handle;
  }

  function retryLater(renew, next = "unreachable") {
    const wait = Math.min(RETRY_BASE_MS * 2 ** retries, RETRY_MAX_MS);
    retries += 1;
    state = next;
    schedule(now() + wait, renew);
  }

  function dead() {
    state = "expired";
    say(`Vault token expired or was rejected (403); ${NEW_TOKEN}.`, "failed");
  }

  function refused() {
    state = "refused";
    say(
      `Vault refused the token lookup (403): the token may be expired, revoked, or its policy lacks auth/token/lookup-self; ${NEW_TOKEN} if reads fail.`,
      "failed",
    );
  }

  /** What a request to Vault came to: ok (with the parsed body when asked;
   * `soft` lets an unreadable body pass as null), dead (the token is
   * refused) or retry (network, timeout, bad reply). */
  async function ask(token, address, method, path, wantBody, soft = false) {
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
      return soft ? { kind: "ok", body: null } : { kind: "retry" };
    }
  }

  /** Looks the token up, renewing it first when asked, and plans the next
   * step: a renewal at about half of the time left, or a notice before a
   * token that cannot be renewed ends. */
  async function run(renew) {
    if (stopped) return;
    const mine = ++epoch;
    stopTimer();
    const { token, unreadable, unavailable } = loadToken();
    if (unreadable) {
      meta = null;
      say(
        `Vault token cannot be read from secure storage; ${NEW_TOKEN}.`,
        "failed",
      );
      return;
    }
    if (unavailable) {
      // Secure storage is not ready yet: not a verdict on the token.
      meta = null;
      return retryLater(renew, "unchecked");
    }
    if (!token) {
      meta = null;
      say(null);
      return;
    }
    const address = getAddress();
    if (!isEnabled()) {
      meta = null;
      state = "unchecked";
      schedule(now() + IDLE_RECHECK_MS, false);
      return;
    }
    if (!address) {
      meta = null;
      state = "unchecked";
      return;
    }
    if (renew && meta?.expiresAt != null) {
      const before = meta.expiresAt;
      const renewed = await ask(
        token,
        address,
        "POST",
        "auth/token/renew-self",
        true,
        true,
      );
      if (mine !== epoch) return;
      if (renewed.kind === "retry") return retryLater(true);
      if (renewed.kind === "dead") cannotRenew = true;
      else {
        const lease = renewed.body?.auth?.lease_duration;
        if (
          isCount(lease) &&
          lease > 0 &&
          now() + lease * 1000 <= before + MOVE_TOLERANCE_MS
        )
          cannotRenew = true;
      }
    }
    const found = await ask(
      token,
      address,
      "GET",
      "auth/token/lookup-self",
      true,
    );
    if (mine !== epoch) return;
    if (found.kind === "dead") {
      // A 403 is also what a token gets when its policy lacks lookup-self,
      // so it is only a death once its end has passed or a read agrees.
      if (meta?.expiresAt != null && now() >= meta.expiresAt) return dead();
      let accepted = null;
      try {
        accepted = await isTokenAccepted(token, address);
      } catch {
        // Nothing to check against: the wording stays honest.
      }
      if (mine !== epoch) return;
      if (accepted === false) return dead();
      if (accepted === null) return refused();
      meta = null;
      retries = 0;
      state = "limited";
      say(null);
      schedule(now() + LIMITED_RECHECK_MS, false);
      return;
    }
    const next = found.kind === "ok" ? metadataOf(found.body, now()) : null;
    if (!next) return retryLater(false);
    if (cannotRenew) next.renewable = false;
    retries = 0;
    meta = next;
    state = "valid";
    if (meta.expiresAt === null) return say(null);
    const left = meta.expiresAt - now();
    if (canRenew(meta)) {
      say(null);
      const half = Math.max(left / 2, MIN_RENEW_DELAY_MS);
      schedule(
        now() + Math.min(half, Math.max(0, left - RENEW_MARGIN_MS)),
        true,
      );
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
    check: () => guarded(run(false)),
    /** The token is gone: forget its state and stop the timer. */
    clear() {
      epoch += 1;
      stopTimer();
      meta = null;
      retries = 0;
      cannotRenew = false;
      state = "unchecked";
      say(null);
    },
    /** Stops the timer for good, for example at quit. */
    stop() {
      stopped = true;
      epoch += 1;
      stopTimer();
    },
    status() {
      const { token, unreadable, unavailable } = loadToken();
      let shown = state;
      if (unreadable) shown = "unreadable";
      else if (!token) shown = unavailable ? "unchecked" : "none";
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
