"""Cookie-based session auth for the web UI.

Reuses the existing MCP Bearer-token model: the user pastes their token at
/ui/login, we validate it with the same TokenConfig the MCP tools use, and
issue a signed HttpOnly cookie. Every UI request re-validates the cookie and
re-resolves the token's TokenEntry so revocations take effect on the next
request.

CSRF posture: SameSite=Lax + cookie-only auth (the UI does not accept the
Bearer header) means cross-site form posts can't reach mutation endpoints
without a same-site request. No CSRF token in v1.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Awaitable, Callable
from urllib.parse import quote

from itsdangerous import BadSignature, SignatureExpired, URLSafeTimedSerializer
from starlette.requests import Request
from starlette.responses import JSONResponse, RedirectResponse, Response

from token_config import TokenConfig, TokenEntry


COOKIE_NAME = "mcp_ui_session"
COOKIE_MAX_AGE = 86400  # 24 hours
COOKIE_PATH = "/ui"
SIGNATURE_SALT = "mcp-ui-session"


@dataclass(frozen=True)
class CurrentUser:
    token: str
    entry: TokenEntry


def make_serializer(secret: str) -> URLSafeTimedSerializer:
    return URLSafeTimedSerializer(secret, salt=SIGNATURE_SALT)


def issue_session_cookie(response: Response, *, secret: str, token: str, secure: bool) -> None:
    serializer = make_serializer(secret)
    payload = serializer.dumps({"token": token})
    response.set_cookie(
        COOKIE_NAME,
        payload,
        max_age=COOKIE_MAX_AGE,
        httponly=True,
        secure=secure,
        samesite="lax",
        path=COOKIE_PATH,
    )


def clear_session_cookie(response: Response) -> None:
    response.delete_cookie(COOKIE_NAME, path=COOKIE_PATH)


def resolve_session(
    request: Request, *, secret: str, token_config: TokenConfig
) -> CurrentUser | None:
    """Verify the session cookie and resolve it to a CurrentUser.

    Returns None if the cookie is missing, tampered, expired, or references a
    token that is no longer valid in token_config.
    """
    raw = request.cookies.get(COOKIE_NAME)
    if not raw:
        return None
    serializer = make_serializer(secret)
    try:
        payload = serializer.loads(raw, max_age=COOKIE_MAX_AGE)
    except (BadSignature, SignatureExpired):
        return None
    token = payload.get("token") if isinstance(payload, dict) else None
    if not token:
        return None
    try:
        entry = token_config.get(token)
    except ValueError:
        return None
    return CurrentUser(token=token, entry=entry)


Handler = Callable[..., Awaitable[Response]]


def login_required_html(handler: Handler) -> Handler:
    """Wraps an HTML handler. Unauthenticated requests redirect to /ui/login."""

    async def wrapper(request: Request) -> Response:
        deps = request.app.state.webui_deps
        user = resolve_session(
            request, secret=deps.ui_secret, token_config=deps.token_config
        )
        if user is None:
            next_path = request.url.path
            if request.url.query:
                next_path = f"{next_path}?{request.url.query}"
            return RedirectResponse(
                url=f"/ui/login?next={quote(next_path, safe='')}", status_code=303
            )
        request.state.current_user = user
        return await handler(request)

    return wrapper


def login_required_api(handler: Handler) -> Handler:
    """Wraps a JSON/htmx mutation handler. Unauthenticated requests get 401."""

    async def wrapper(request: Request) -> Response:
        deps = request.app.state.webui_deps
        user = resolve_session(
            request, secret=deps.ui_secret, token_config=deps.token_config
        )
        if user is None:
            return JSONResponse({"error": "unauthorized"}, status_code=401)
        request.state.current_user = user
        return await handler(request)

    return wrapper
