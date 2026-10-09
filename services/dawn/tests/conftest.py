from __future__ import annotations

import importlib.util
import pathlib
import sys
import types

import pytest

PYTHON_SOURCE = pathlib.Path(__file__).resolve().parents[3] / "dusk/src/dusk_py/python"

if importlib.util.find_spec("dusk") is None:
    sys.path.insert(0, str(PYTHON_SOURCE))
    sys.modules["dusk.dusk"] = types.ModuleType("dusk.dusk")


@pytest.fixture
def anyio_backend() -> str:
    return "asyncio"


class CertificateAuthority:
    def __init__(self, name: str = "dusk internal test CA") -> None:
        import datetime

        from cryptography import x509
        from cryptography.hazmat.primitives import hashes
        from cryptography.hazmat.primitives.asymmetric import ec
        from cryptography.x509.oid import NameOID

        self.key = ec.generate_private_key(ec.SECP256R1())
        subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, name)])
        now = datetime.datetime.now(datetime.UTC)
        self.certificate = (
            x509.CertificateBuilder()
            .subject_name(subject)
            .issuer_name(subject)
            .public_key(self.key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(minutes=5))
            .not_valid_after(now + datetime.timedelta(days=1))
            .add_extension(
                x509.BasicConstraints(ca=True, path_length=None), critical=True
            )
            .add_extension(
                x509.SubjectKeyIdentifier.from_public_key(self.key.public_key()),
                critical=False,
            )
            .add_extension(
                x509.KeyUsage(
                    digital_signature=True,
                    content_commitment=False,
                    key_encipherment=False,
                    data_encipherment=False,
                    key_agreement=False,
                    key_cert_sign=True,
                    crl_sign=True,
                    encipher_only=False,
                    decipher_only=False,
                ),
                critical=True,
            )
            .sign(self.key, hashes.SHA256())
        )

    def issue(
        self,
        uris: tuple[str, ...] = (),
        dns_names: tuple[str, ...] = (),
        ip_addresses: tuple[str, ...] = (),
        server: bool = False,
    ):
        import datetime
        import ipaddress

        from cryptography import x509
        from cryptography.hazmat.primitives import hashes
        from cryptography.hazmat.primitives.asymmetric import ec
        from cryptography.x509.oid import ExtendedKeyUsageOID

        key = ec.generate_private_key(ec.SECP256R1())
        now = datetime.datetime.now(datetime.UTC)
        names: list[x509.GeneralName] = [
            x509.UniformResourceIdentifier(uri) for uri in uris
        ]
        names += [x509.DNSName(name) for name in dns_names]
        names += [
            x509.IPAddress(ipaddress.ip_address(address)) for address in ip_addresses
        ]
        builder = (
            x509.CertificateBuilder()
            .subject_name(x509.Name([]))
            .issuer_name(self.certificate.subject)
            .public_key(key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(minutes=5))
            .not_valid_after(now + datetime.timedelta(hours=24))
            .add_extension(
                x509.ExtendedKeyUsage(
                    [
                        ExtendedKeyUsageOID.SERVER_AUTH
                        if server
                        else ExtendedKeyUsageOID.CLIENT_AUTH
                    ]
                ),
                critical=False,
            )
            .add_extension(
                x509.SubjectKeyIdentifier.from_public_key(key.public_key()),
                critical=False,
            )
            .add_extension(
                x509.AuthorityKeyIdentifier.from_issuer_public_key(
                    self.key.public_key()
                ),
                critical=False,
            )
        )
        if names:
            builder = builder.add_extension(
                x509.SubjectAlternativeName(names), critical=True
            )
        return key, builder.sign(self.key, hashes.SHA256())

    def write(self, directory, stem: str, key, certificate) -> tuple[str, str]:
        from cryptography.hazmat.primitives import serialization

        key_path = directory / f"{stem}.key"
        certificate_path = directory / f"{stem}.crt"
        key_path.write_bytes(
            key.private_bytes(
                serialization.Encoding.PEM,
                serialization.PrivateFormat.PKCS8,
                serialization.NoEncryption(),
            )
        )
        certificate_path.write_bytes(
            certificate.public_bytes(serialization.Encoding.PEM)
        )
        return str(certificate_path), str(key_path)

    def write_root(self, directory) -> str:
        from cryptography.hazmat.primitives import serialization

        path = directory / "ca.crt"
        path.write_bytes(self.certificate.public_bytes(serialization.Encoding.PEM))
        return str(path)


def der(certificate) -> bytes:
    from cryptography.hazmat.primitives import serialization

    return certificate.public_bytes(serialization.Encoding.DER)
