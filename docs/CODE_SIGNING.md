# Code Signing and Smart App Control

Self-signing this app for local use, and the Windows 11 Smart App Control friction that comes with running a self-signed executable. See the main [README](../README.md) for everything else.

Windows 11 Smart App Control (SAC) blocks unsigned executables outright — no "Run anyway" override, unlike classic SmartScreen. Every new build needs re-signing, since each build is a new file hash.

### Current certificate

- Subject: `CN=Sticky Notes Lite (Local)` (or whatever you choose when creating your own)
- Thumbprint: `<YOUR_CERTIFICATE_THUMBPRINT>` -- find yours with `Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert`
- Store: `Cert:\CurrentUser\My` (private key), trusted via `LocalMachine\Root` + `LocalMachine\TrustedPublisher`
- Validity is whatever you pass to `-NotAfter` when creating it (see below); 10 years is a reasonable default

`build-release.ps1` does this for you. To sign by hand:

```powershell
# SDK versions differ machine to machine -- resolve the newest installed signtool.exe
# rather than hardcoding a version number (this is what build-release.ps1 itself does).
$signtool = (Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName | Select-Object -Last 1).FullName
& $signtool sign /fd SHA256 /sha1 "<YOUR_CERTIFICATE_THUMBPRINT>" /tr http://timestamp.digicert.com /td SHA256 "src-tauri\target\release\sticky-notes-lite.exe"
```

Then copy the signed exe to `dist\` — this is the stable location to actually run/autostart from; don't run directly out of `target\release\`, since that gets wiped whenever `target\` is cleaned up:

```powershell
Copy-Item "src-tauri\target\release\sticky-notes-lite.exe" "dist\sticky-notes-lite.exe" -Force
```

### If the certificate is ever lost and needs recreating

Two ways to generate a fresh signing certificate — either works; the PowerShell method below is what's currently in use.

**Option A — `New-SelfSignedCertificate` (PowerShell, no extra tools needed):**

```powershell
$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Sticky Notes Lite (Local)" `
    -CertStoreLocation "Cert:\CurrentUser\My" -NotAfter (Get-Date).AddYears(10) `
    -KeyUsage DigitalSignature -KeyAlgorithm RSA -KeyLength 2048
$cert.Thumbprint   # note this down — you'll need it for every future signtool call

Export-Certificate -Cert $cert -FilePath "sticky-notes-lite-signing.cer"
```

Then, in an **elevated** PowerShell (adding to the Root store from a script requires elevation and, even then, `Import-Certificate` throws "UI is not allowed" — use `certutil` instead, which doesn't hit that restriction):

```powershell
certutil -f -addstore "Root" "sticky-notes-lite-signing.cer"
certutil -f -addstore "TrustedPublisher" "sticky-notes-lite-signing.cer"
```

**Option B — `mkcert` + OpenSSL (if you'd rather use an existing local dev CA):**

[mkcert](https://github.com/FiloSottile/mkcert) is a tool for generating locally-trusted certificates — normally for HTTPS/TLS on localhost, not code signing, but its local CA can be reused to issue a code-signing certificate too, via OpenSSL. If you already use `mkcert` for local HTTPS dev, you likely have both `mkcert` and OpenSSL installed already and a CA already trusted -- skip to the OpenSSL steps below. Otherwise, starting from scratch:

```powershell
# Install mkcert (pick one)
winget install FiloSottile.mkcert
# or: choco install mkcert
# or download the binary from https://github.com/FiloSottile/mkcert/releases and put it on PATH

# Create + trust a local CA (installs into Windows' Root store, and Firefox's if present)
mkcert -install

# Find where the CA files live
mkcert -CAROOT
# -> e.g. C:\Users\<you>\AppData\Local\mkcert\rootCA.pem / rootCA-key.pem
```

mkcert itself only issues TLS server/client certs, not code-signing ones — to get a codeSigning-EKU leaf certificate chained to that already-trusted CA, use OpenSSL directly against the CA files mkcert created:

```powershell
cd (mkcert -CAROOT)   # or wherever rootCA.pem / rootCA-key.pem live

# 1. Generate a private key for the new signing cert
openssl genrsa -out codesign-key.pem 2048

# 2. Write a minimal OpenSSL config requesting the codeSigning EKU
@'
[req]
distinguished_name = dn
req_extensions = ext
prompt = no

[dn]
CN = Sticky Notes Lite (mkcert)

[ext]
keyUsage = digitalSignature
extendedKeyUsage = codeSigning
'@ | Set-Content codesign.cnf

# 3. Create the CSR
openssl req -new -key codesign-key.pem -out codesign.csr -config codesign.cnf

# 4. Sign it with the mkcert CA (produces a leaf cert chained to the already-trusted CA)
openssl x509 -req -in codesign.csr -CA rootCA.pem -CAkey rootCA-key.pem -CAcreateserial `
    -out codesign-cert.pem -days 3650 -extfile codesign.cnf -extensions ext

# 5. Bundle into a .pfx (PowerShell/signtool need the private key alongside the cert)
openssl pkcs12 -export -out codesign.pfx -inkey codesign-key.pem -in codesign-cert.pem -passout pass:changeit

# 6. Import into your personal certificate store
Import-PfxCertificate -FilePath codesign.pfx -CertStoreLocation Cert:\CurrentUser\My `
    -Password (ConvertTo-SecureString -String "changeit" -AsPlainText -Force)
```

Since the mkcert CA is already trusted machine-wide, this leaf certificate is trusted too without any further `certutil` step — just grab its thumbprint (`Get-PfxCertificate codesign.pfx`) and use it with `signtool sign /sha1 <thumbprint> ...` the same way as Option A.

**Caveat that applies to *either* option**: see the next section — Smart App Control's acceptance of *any* self-signed/locally-trusted certificate (mkcert-based or not) has proven unpredictable in practice. Neither method produces a certificate that reaches the "Enterprise" Code Integrity signing level Windows actually wants; both just happen to work often enough to be worth using over nothing.

### Smart App Control considerations

The exact same signing recipe (same certificate, same elevated PowerShell) has both succeeded and failed across different builds of this app. Checking the reason via:

```powershell
Get-WinEvent -LogName "Microsoft-Windows-CodeIntegrity/Operational" -MaxEvents 10 |
    Where-Object { $_.Message -match "sticky-notes-lite" } | Select-Object TimeCreated, Id, Message
```

...consistently shows:

```
Code Integrity determined that a process ... attempted to load sticky-notes-lite.exe
that did not meet the Enterprise signing level requirements or violated code
integrity policy (Policy ID: {0283ac0f-fff1-49ae-ada1-8a933130cad6}).
```

"Enterprise signing level" is a specific Windows Code Integrity classification that a self-signed certificate — from `New-SelfSignedCertificate`, mkcert, or otherwise — cannot structurally satisfy on its own. It appears SAC sometimes grants temporary leniency to a brand-new file hash from an already-trusted signer, and sometimes doesn't; this is not about shell choice or elevation (both have been tried in both winning and losing combinations).

**If a freshly signed build gets blocked**, the fix that has worked here: turn Smart App Control off, then back on (Settings → Privacy & security → Windows Security → App & browser control → Smart App Control). This clears the stuck per-file block. Contrary to older guidance, this does **not** require a full Windows reinstall/reset — toggling it off and back on in place has worked. On some systems, clearing and re-enabling Smart App Control this way has resolved stale policy decisions, although behavior can vary between Windows versions and builds.

**Reliable fallback while troubleshooting**: `npm run tauri dev` has never once hit this issue across the entire development of this app, since it's launched via `cargo run` rather than as a standalone signed binary.

**On another machine**: whether this friction shows up at all depends on whether *that* machine has Smart App Control enabled (it's opt-in even on clean Windows 11 installs). Confirmed on a second laptop without SAC: the exe just shows a normal SmartScreen "unrecognized publisher" prompt with a working "Run anyway" — no certificate setup needed there at all. If SAC-blocked friction on other machines becomes a recurring problem, look into **Azure Trusted Signing** (~$10/month, a real broadly-recognized certificate without the overhead of Microsoft Store submission) rather than continuing to fight self-signing per machine.
