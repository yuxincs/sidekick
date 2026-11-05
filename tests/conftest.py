import asyncio
import pytest

DEVICE_ID = "01234567-89ab-4def-8123-456789abcdef"


def make_snapshot(**exposure):
    return {
        "type": "snapshot",
        "data": {
            "protocol": 1,
            "device": {"id": DEVICE_ID, "name": "Test desktop", "os": "test", "version": "0.2.0"},
            "exposure": {"cpu": True, "memory": True, "shutdown": False, **exposure},
            "metrics": {"cpu_percent": 42.0, "memory_percent": 61.0},
        },
    }


@pytest.fixture(autouse=True)
def custom_integrations(enable_custom_integrations):
    yield


async def eventually(predicate, timeout=5):
    async with asyncio.timeout(timeout):
        while not predicate():
            await asyncio.sleep(0.01)


@pytest.fixture
async def pairing_server(hass, aiohttp_server, socket_enabled, request, tmp_path):
    from homeassistant.setup import async_setup_component
    from homeassistant.components.http import KEY_HASS
    from custom_components.sidekick.pairing import async_get_manager, PairView, SessionView
    from aiohttp import web

    assert await async_setup_component(hass, "http", {})
    manager = await async_get_manager(hass)
    manager.allow_insecure_loopback_for_tests = True
    app = web.Application()
    app[KEY_HASS] = hass
    app.router.add_get("/api/sidekick/pair", PairView(manager).get)
    app.router.add_get("/api/sidekick/ws", SessionView(manager).get)
    tls = getattr(request, "param", None) == "tls"
    if tls:
        import ssl
        import ipaddress
        from datetime import datetime, timedelta, timezone
        from cryptography import x509
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric import rsa
        from cryptography.x509.oid import NameOID, ExtendedKeyUsageOID

        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Sidekick test CA")])
        cert = (
            x509.CertificateBuilder()
            .subject_name(subject)
            .issuer_name(subject)
            .public_key(key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(datetime.now(timezone.utc) - timedelta(days=1))
            .not_valid_after(datetime.now(timezone.utc) + timedelta(days=1))
            .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
            .add_extension(
                x509.SubjectAlternativeName([x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False
            )
            .sign(key, hashes.SHA256())
        )
        certificate = tmp_path / "ca.pem"
        certificate.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
        server_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        server_subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Sidekick test server")])
        leaf = (
            x509.CertificateBuilder()
            .subject_name(server_subject)
            .issuer_name(subject)
            .public_key(server_key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(datetime.now(timezone.utc) - timedelta(days=1))
            .not_valid_after(datetime.now(timezone.utc) + timedelta(days=1))
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(
                x509.SubjectAlternativeName([x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False
            )
            .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
            .sign(key, hashes.SHA256())
        )
        leaf_path = tmp_path / "server.pem"
        leaf_path.write_bytes(leaf.public_bytes(serialization.Encoding.PEM))
        private = tmp_path / "key.pem"
        private.write_bytes(
            server_key.private_bytes(
                serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()
            )
        )
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(leaf_path, private)
        manager.allow_insecure_loopback_for_tests = False
        server = await aiohttp_server(app, ssl=context)
        server.trust_ca = str(certificate)
    else:
        server = await aiohttp_server(app)
    yield manager, server
    await manager.stop()


@pytest.fixture
async def http_client(socket_enabled):
    import aiohttp

    async with aiohttp.ClientSession() as client:
        yield client
