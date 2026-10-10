alter table nodes
    add column credential_kind text collate "C" check (credential_kind in ('fleet_token', 'install_token', 'tpm_attestation')),
    add column credential_ref text collate "C",
    add column credential_issuer text collate "C";

create index nodes_credential_ref on nodes (credential_kind, credential_ref, device_id, installation_id);
create index nodes_credential_issuer on nodes (credential_issuer, device_id, installation_id) where credential_issuer is not null;
