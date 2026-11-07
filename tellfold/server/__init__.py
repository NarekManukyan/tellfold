"""Tellfold Studio server: `create_app(runs_dir, llm_factory)` returns the FastAPI app behind `tellfold ui`."""

from tellfold.server.app import create_app

__all__ = ["create_app"]
