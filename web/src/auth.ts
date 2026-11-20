export interface CungChatUser {
  id: string;
  email: string;
  role?: string;
  emailVerified?: boolean;
}

const TOKEN_STORAGE_KEY = 'greenc_cc_access_token';
export const ACCOUNT_API_BASE =
  (import.meta.env.VITE_ACCOUNT_API_BASE as string | undefined) || 'https://account.cungchat.com';

export function getStoredToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function setStoredToken(token: string | null): void {
  try {
    if (token) {
      localStorage.setItem(TOKEN_STORAGE_KEY, token);
    } else {
      localStorage.removeItem(TOKEN_STORAGE_KEY);
    }
  } catch {
    // Ignore storage quota / private mode failures
  }
}

/**
 * Capture `?accessToken=...` returned by `account.cungchat.com/auth/google`
 * on `/auth/callback` (or root) and immediately scrub the token from the URL bar.
 */
export function consumeOAuthCallbackFromUrl(): string | null {
  const url = new URL(window.location.href);
  const token = url.searchParams.get('accessToken');
  if (!token) return null;

  setStoredToken(token);
  url.searchParams.delete('accessToken');
  const cleanPath = url.pathname === '/auth/callback' ? '/' : url.pathname;
  const cleanUrl = cleanPath + (url.searchParams.toString() ? `?${url.searchParams.toString()}` : '') + url.hash;
  window.history.replaceState({}, document.title, cleanUrl);
  return token;
}

export function buildGoogleLoginUrl(): string {
  const redirectUri = `${window.location.origin}/auth/callback`;
  return `${ACCOUNT_API_BASE}/auth/google?redirect_uri=${encodeURIComponent(redirectUri)}`;
}

export async function fetchCurrentUser(token: string): Promise<CungChatUser | null> {
  try {
    const res = await fetch(`${ACCOUNT_API_BASE}/users/me`, {
      headers: {
        Authorization: `Bearer ${token}`,
      },
    });
    if (!res.ok) return null;
    const data = (await res.json()) as Partial<CungChatUser>;
    if (!data.email) return null;
    return {
      id: String(data.id ?? ''),
      email: data.email,
      role: data.role,
      emailVerified: data.emailVerified,
    };
  } catch {
    return null;
  }
}

export async function loginWithEmail(email: string, password: string): Promise<string> {
  const res = await fetch(`${ACCOUNT_API_BASE}/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ email: email.trim(), password }),
  });
  const body = (await res.json().catch(() => ({}))) as { accessToken?: string; message?: string | string[] };
  if (!res.ok || !body.accessToken) {
    const msg = Array.isArray(body.message) ? body.message.join(', ') : body.message;
    throw new Error(msg || 'INVALID_CREDENTIALS');
  }
  setStoredToken(body.accessToken);
  return body.accessToken;
}

export async function registerWithEmail(email: string, password: string): Promise<string> {
  const res = await fetch(`${ACCOUNT_API_BASE}/auth/register`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      email: email.trim(),
      password,
      redirectUri: `${window.location.origin}/`,
    }),
  });
  const body = (await res.json().catch(() => ({}))) as { accessToken?: string; message?: string | string[] };
  if (!res.ok || !body.accessToken) {
    const msg = Array.isArray(body.message) ? body.message.join(', ') : body.message;
    throw new Error(msg || 'REGISTER_FAILED');
  }
  setStoredToken(body.accessToken);
  return body.accessToken;
}

export async function requestVerifiedDownload(token: string): Promise<{ downloadUrl: string; filename: string }> {
  const res = await fetch('/api/download-ticket', {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
  });
  if (res.status === 401 || res.status === 403) {
    setStoredToken(null);
    throw new Error('UNAUTHORIZED');
  }
  if (!res.ok) {
    throw new Error('DOWNLOAD_ERROR');
  }
  const data = (await res.json()) as { ticket: string; filename: string };
  return {
    downloadUrl: `/api/download?ticket=${encodeURIComponent(data.ticket)}`,
    filename: data.filename,
  };
}
