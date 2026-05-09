import httpx


async def test_static_app_css_served(client: httpx.AsyncClient):
    response = await client.get("/ui/static/app.css")
    assert response.status_code == 200
    assert "text/css" in response.headers["content-type"]
    assert ":root" in response.text


async def test_static_htmx_js_served(client: httpx.AsyncClient):
    response = await client.get("/ui/static/htmx.min.js")
    assert response.status_code == 200
    # mimetypes maps .js to text/javascript or application/javascript depending on platform.
    assert "javascript" in response.headers["content-type"]


async def test_static_unknown_file_404(client: httpx.AsyncClient):
    response = await client.get("/ui/static/does-not-exist.css")
    assert response.status_code == 404


async def test_static_path_traversal_rejected(client: httpx.AsyncClient):
    # Even if Starlette decodes %2e%2e, our handler must reject paths that
    # escape the static directory.
    response = await client.get("/ui/static/..%2F..%2Fserver.py")
    assert response.status_code == 404
