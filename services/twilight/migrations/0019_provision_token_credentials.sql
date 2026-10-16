alter table nodes
    drop constraint nodes_credential_kind_check,
    add constraint nodes_credential_kind_check
        check (credential_kind in ('fleet_token', 'install_token', 'provision_token', 'tpm_attestation')) not valid;

alter table revocations
    drop constraint revocations_credential_kind_check,
    add constraint revocations_credential_kind_check
        check (credential_kind in ('fleet_token', 'install_token', 'provision_token')) not valid;
