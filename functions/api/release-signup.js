import { handleReleaseSignup } from '../../docs/server/release-signup.mjs';

export async function onRequest({ request, env }) {
  const allowedOrigin = env.RELEASE_SIGNUP_ORIGIN ?? 'https://teamctl.run';
  if (new URL(request.url).origin !== allowedOrigin) {
    return new Response(JSON.stringify({ success: false, error: 'Could not save your signup. Please try again.' }), {
      status: 403,
      headers: { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' },
    });
  }
  const upstreamStatuses = [];
  const response = await handleReleaseSignup(request, env, {
    fetchImpl: async (url, options) => {
      const upstream = await fetch(url, options);
      upstreamStatuses.push(upstream.status);
      return upstream;
    },
  });
  if (response.status >= 500) {
    // Operational diagnostics contain no contact details, URLs or credentials.
    console.warn({ event: 'release_signup_failed', status: response.status, upstreamStatuses });
  }
  return response;
}
