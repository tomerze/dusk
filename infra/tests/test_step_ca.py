import datetime
import json
import os
import secrets
import ssl
import tempfile
import time
import urllib.error
import urllib.request

import jwt
import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

STEP_CA_URL = os.environ.get("STEP_CA_URL", "")
STEP_CA_ROOT = os.environ.get("STEP_CA_ROOT", "")
PROVISIONER_KEY = os.environ.get("PROVISIONER_KEY", "")
CERTIFICATE_LIFETIME = datetime.timedelta(hours=168)

pytestmark = pytest.mark.skipif(
    not (STEP_CA_URL and STEP_CA_ROOT and PROVISIONER_KEY),
    reason="STEP_CA_URL, STEP_CA_ROOT and PROVISIONER_KEY name a running step-ca",
)


def device_uri(device_id: str) -> str:
    return f"urn:dusk:device:{device_id}"


def installation_uri(installation_id: str) -> str:
    return f"urn:dusk:installation:{installation_id}"


def certificate_request(
    uris: list[str],
    common_name: str = "",
    key: ec.EllipticCurvePrivateKey | None = None,
) -> bytes:
    key = key or ec.generate_private_key(ec.SECP256R1())
    attributes = (
        [x509.NameAttribute(NameOID.COMMON_NAME, common_name, _validate=False)]
        if common_name
        else []
    )
    request = (
        x509.CertificateSigningRequestBuilder()
        .subject_name(x509.Name(attributes))
        .add_extension(
            x509.SubjectAlternativeName(
                [x509.UniformResourceIdentifier(uri) for uri in uris]
            ),
            critical=False,
        )
        .sign(key, hashes.SHA256())
    )
    return request.public_bytes(serialization.Encoding.PEM)


def one_time_token(
    subject: str, uris: list[str], claims: dict[str, object] | None = None
) -> str:
    with open(PROVISIONER_KEY) as file:
        provisioner = json.load(file)
    signing_key = jwt.PyJWK(provisioner, algorithm="ES256").key
    now = int(time.time())
    payload: dict[str, object] = {
        "iss": "nightfall",
        "aud": f"{STEP_CA_URL}/1.0/sign",
        "sub": subject,
        "sans": uris,
        "iat": now,
        "nbf": now,
        "exp": now + 300,
        "jti": secrets.token_hex(32),
    }
    payload.update(claims or {})
    return jwt.encode(
        payload, signing_key, algorithm="ES256", headers={"kid": provisioner["kid"]}
    )


def sign(
    csr: bytes, token: str, lifetime: str = "168h"
) -> tuple[int, dict[str, object]]:
    context = ssl.create_default_context(cafile=STEP_CA_ROOT)
    body = json.dumps(
        {"csr": csr.decode(), "ott": token, "notAfter": lifetime}
    ).encode()
    request = urllib.request.Request(
        f"{STEP_CA_URL}/1.0/sign",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, context=context, timeout=10) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as failure:
        return failure.code, json.load(failure)


def renew(certificate_chain: str, key: ec.EllipticCurvePrivateKey) -> int:
    context = ssl.create_default_context(cafile=STEP_CA_ROOT)
    with tempfile.TemporaryDirectory() as directory:
        chain_path = os.path.join(directory, "chain.crt")
        key_path = os.path.join(directory, "node.key")
        with open(chain_path, "w") as file:
            file.write(certificate_chain)
        with open(key_path, "wb") as file:
            file.write(
                key.private_bytes(
                    serialization.Encoding.PEM,
                    serialization.PrivateFormat.PKCS8,
                    serialization.NoEncryption(),
                )
            )
        context.load_cert_chain(chain_path, key_path)
    request = urllib.request.Request(
        f"{STEP_CA_URL}/1.0/renew", data=b"", method="POST"
    )
    try:
        with urllib.request.urlopen(request, context=context, timeout=10) as response:
            return response.status
    except urllib.error.HTTPError as failure:
        return failure.code


def identity() -> tuple[str, str]:
    return secrets.token_hex(16), secrets.token_hex(16)


def issued_certificate(response: dict[str, object]) -> x509.Certificate:
    return x509.load_pem_x509_certificate(str(response["crt"]).encode())


def test_signs_a_node_request_and_adds_the_tenant() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    status, response = sign(
        certificate_request(uris),
        one_time_token(f"{device_id}.{installation_id}", uris, {"tenant": "acme"}),
    )
    assert status == 201, response
    certificate = issued_certificate(response)
    alternative_names = certificate.extensions.get_extension_for_class(
        x509.SubjectAlternativeName
    ).value
    assert sorted(
        alternative_names.get_values_for_type(x509.UniformResourceIdentifier)
    ) == sorted([*uris, "urn:dusk:tenant:acme"])
    assert alternative_names.get_values_for_type(x509.DNSName) == []
    assert alternative_names.get_values_for_type(x509.IPAddress) == []
    assert alternative_names.get_values_for_type(x509.RFC822Name) == []
    usages = certificate.extensions.get_extension_for_class(x509.ExtendedKeyUsage).value
    assert list(usages) == [ExtendedKeyUsageOID.CLIENT_AUTH]
    key_usage = certificate.extensions.get_extension_for_class(x509.KeyUsage).value
    assert key_usage.digital_signature
    assert not key_usage.key_encipherment
    assert not key_usage.key_cert_sign
    assert len(certificate.subject) == 0
    assert certificate.extensions.get_extension_for_class(
        x509.SubjectAlternativeName
    ).critical
    remaining = certificate.not_valid_after_utc - datetime.datetime.now(datetime.UTC)
    assert (
        CERTIFICATE_LIFETIME - datetime.timedelta(minutes=2)
        < remaining
        <= CERTIFICATE_LIFETIME
    )


def test_signs_a_request_whose_common_name_is_the_identity_without_a_tenant() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    common_name = f"{device_id}.{installation_id}"
    status, response = sign(
        certificate_request(uris, common_name), one_time_token(common_name, uris)
    )
    assert status == 201, response
    certificate = issued_certificate(response)
    alternative_names = certificate.extensions.get_extension_for_class(
        x509.SubjectAlternativeName
    )
    assert sorted(
        alternative_names.value.get_values_for_type(x509.UniformResourceIdentifier)
    ) == sorted(uris)
    assert len(certificate.subject) == 0


def test_refuses_a_token_used_twice() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    token = one_time_token(f"{device_id}.{installation_id}", uris)
    first, response = sign(certificate_request(uris), token)
    assert first == 201, response
    second, response = sign(certificate_request(uris), token)
    assert second == 401, response


@pytest.mark.parametrize(
    "uris",
    [
        pytest.param(
            [
                device_uri("a" * 32),
                installation_uri("b" * 32),
                "urn:dusk:principal:dawn-0",
            ],
            id="extra uri",
        ),
        pytest.param(
            [device_uri("a" * 32), device_uri("c" * 32), installation_uri("b" * 32)],
            id="two devices",
        ),
        pytest.param([device_uri("a" * 32)], id="no installation"),
        pytest.param(
            [device_uri("A" * 32), installation_uri("b" * 32)], id="uppercase device id"
        ),
        pytest.param(
            [device_uri("a" * 31), installation_uri("b" * 32)], id="short device id"
        ),
    ],
)
def test_refuses_requests_that_are_not_exactly_one_device_and_one_installation(
    uris: list[str],
) -> None:
    status, response = sign(certificate_request(uris), one_time_token("node", uris))
    assert status in (400, 403), response


def test_refuses_a_common_name_other_than_the_identity() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    status, response = sign(
        certificate_request(uris, "someone-else"), one_time_token("someone-else", uris)
    )
    assert status in (400, 403), response


def test_adds_the_tpm_attestation_marker() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    status, response = sign(
        certificate_request(uris),
        one_time_token(
            f"{device_id}.{installation_id}",
            uris,
            {"tenant": "acme", "attestation": "tpm"},
        ),
    )
    assert status == 201, response
    alternative_names = (
        issued_certificate(response)
        .extensions.get_extension_for_class(x509.SubjectAlternativeName)
        .value
    )
    assert sorted(
        alternative_names.get_values_for_type(x509.UniformResourceIdentifier)
    ) == sorted([*uris, "urn:dusk:tenant:acme", "urn:dusk:attestation:tpm"])


def test_refuses_an_attestation_claim_other_than_tpm() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    token = one_time_token(
        f"{device_id}.{installation_id}", uris, {"attestation": "sgx"}
    )
    status, response = sign(certificate_request(uris), token)
    assert status in (400, 403), response


def test_refuses_a_malformed_tenant() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    token = one_time_token(
        f"{device_id}.{installation_id}", uris, {"tenant": "Not_A_Tenant"}
    )
    status, response = sign(certificate_request(uris), token)
    assert status in (400, 403), response


def test_refuses_a_lifetime_beyond_the_provisioner_maximum() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    token = one_time_token(f"{device_id}.{installation_id}", uris)
    status, response = sign(certificate_request(uris), token, lifetime="200h")
    assert status in (400, 403), response


def test_refuses_a_token_signed_by_another_key() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    with open(PROVISIONER_KEY) as file:
        kid = json.load(file)["kid"]
    stranger = ec.generate_private_key(ec.SECP256R1())
    now = int(time.time())
    token = jwt.encode(
        {
            "iss": "nightfall",
            "aud": f"{STEP_CA_URL}/1.0/sign",
            "sub": f"{device_id}.{installation_id}",
            "sans": uris,
            "iat": now,
            "nbf": now,
            "exp": now + 300,
            "jti": secrets.token_hex(32),
        },
        stranger,
        algorithm="ES256",
        headers={"kid": kid},
    )
    status, response = sign(certificate_request(uris), token)
    assert status == 401, response


def test_refuses_a_request_whose_identity_differs_from_the_token() -> None:
    device_id, installation_id = identity()
    other_device_id, other_installation_id = identity()
    status, response = sign(
        certificate_request(
            [device_uri(other_device_id), installation_uri(other_installation_id)]
        ),
        one_time_token(
            f"{device_id}.{installation_id}",
            [device_uri(device_id), installation_uri(installation_id)],
        ),
    )
    assert status in (400, 401, 403), response


def test_refuses_to_renew_a_node_certificate() -> None:
    device_id, installation_id = identity()
    uris = [device_uri(device_id), installation_uri(installation_id)]
    key = ec.generate_private_key(ec.SECP256R1())
    status, response = sign(
        certificate_request(uris, key=key),
        one_time_token(f"{device_id}.{installation_id}", uris),
    )
    assert status == 201, response
    certificates = response["certChain"]
    assert isinstance(certificates, list)
    assert renew("".join(map(str, certificates)), key) in (401, 403)
