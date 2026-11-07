"""Bearer-token guard for `tellfold serve`: every `/api` request must carry `Authorization: Bearer <token>`."""

from __future__ import annotations

import hmac

from starlette.responses import JSONResponse
from starlette.types import ASGIApp, Receive, Scope, Send


class TokenAuthMiddleware:
    """Constant-time comparison of the whole header value; anything else on `/api` gets 401."""

    def __init__(self, app: ASGIApp, token: str, prefix: str = "/api"):
        if not token:
            raise ValueError("an empty token would disable authentication")
        self.app = app
        self.expected = f"Bearer {token}".encode("latin-1")
        self.prefix = prefix

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] == "http":
            path = scope.get("path", "")
            if path == self.prefix or path.startswith(self.prefix + "/"):
                supplied = b""
                for key, value in scope.get("headers", []):
                    if key.lower() == b"authorization":
                        supplied = value
                        break
                if not hmac.compare_digest(supplied, self.expected):
                    response = JSONResponse({"detail": "unauthorized"}, status_code=401,
                                            headers={"WWW-Authenticate": "Bearer"})
                    await response(scope, receive, send)
                    return
        await self.app(scope, receive, send)
