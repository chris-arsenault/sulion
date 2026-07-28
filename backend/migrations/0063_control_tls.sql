-- TLS identity for the node channel.
--
-- Node traffic carries terminal bytes — credentials included — so the LAN hop
-- must be encrypted. The control host grants no elevated network privileges,
-- so TLS terminates in the control process itself with a self-generated
-- certificate. Nodes pin it on first pairing, and the Ed25519 handshake proof
-- signs its digest so the TLS layer is bound to the identity an operator
-- approved. Stored beside control_identity so it survives redeploys.

CREATE TABLE control_tls (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    private_key_pem TEXT NOT NULL,
    cert_pem TEXT NOT NULL,
    -- SANs the certificate was minted for; a change regenerates it.
    subject_alt_names TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
