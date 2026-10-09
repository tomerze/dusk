@0xf3f0eb0aa0edc453;

using Dusk = import "/capnp/dusk.capnp";

struct Credential {
  union {
    fleetToken @0 :Text $Dusk.sensitive;
    installToken @1 :Text $Dusk.sensitive;
    tpmAttestation @2 :TpmAttestation;
    certificate @3 :Void;
  }
}

struct TpmAttestation {
  endorsementKey @0 :Data;
  attestationKey @1 :Data;
  quote @2 :Data;
  signature @3 :Data;
  certifyInfo @4 :Data;
}

struct DeviceReport {
  hardwareFingerprint @0 :Data;
  installationHint @1 :Text;
  duskVersion @2 :Text;
  impl @3 :Text;
  targetOs @4 :Text;
  targetArch @5 :Text;
  hostname @6 :Text;
}

struct Assignment {
  deviceId @0 :Text;
  installationId @1 :Text;
  challenge @2 :Data;
  challengeExpiresUnixMs @3 :UInt64;
}

struct Issued {
  certificateChain @0 :List(Data);
  notAfterUnixMs @1 :UInt64;
  renewAfterUnixMs @2 :UInt64;
}

interface Provisioning {
  assign @0 (credential :Credential, device :DeviceReport) -> (assignment :Assignment);
  enroll @1 (credential :Credential, device :DeviceReport, challenge :Data, csr :Data) -> (issued :Issued);
  renew @2 (csr :Data) -> (issued :Issued);
}

const renewBeyondGrace :Text = "renew refused: the certificate expired beyond the renew grace";
