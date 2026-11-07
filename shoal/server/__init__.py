"""Shoal Studio server: `create_app(runs_dir, llm_factory)` returns the FastAPI app behind `shoal ui`."""

from shoal.server.app import create_app

__all__ = ["create_app"]
