namespace Networker.Endpoint;

/// <summary>
/// Static HTML head/foot for the landing page, copied verbatim from the Rust
/// <c>LANDING_HTML_HEAD</c> / <c>LANDING_HTML_FOOT</c> constants.
/// </summary>
internal static class Landing
{
    public const string Head = """
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="icon" href="data:,">
<style>
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:system-ui,-apple-system,sans-serif;background:#0f1117;color:#e8e8e8;padding:2rem 2.5rem;max-width:940px}
h1{font-size:1.6rem;color:#fff;font-weight:700}
.meta{color:#7a9aaa;font-size:.85rem;margin:.3rem 0 1.2rem}
.status{display:inline-flex;align-items:center;gap:.4rem;background:#1b3a1b;color:#4caf50;border:1px solid #2e5a2e;padding:.25rem .8rem;border-radius:20px;font-size:.8rem;font-weight:600;margin-bottom:1.5rem}
.dot{width:7px;height:7px;background:#4caf50;border-radius:50%;animation:pulse 1.5s ease-in-out infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(240px,1fr));gap:1rem;margin-bottom:1.5rem}
.card{background:#1a1a2e;border:1px solid #2a2a40;border-radius:10px;padding:1rem 1.2rem}
.full{margin-bottom:1.5rem}
.card-title{font-size:.7rem;text-transform:uppercase;letter-spacing:.08em;color:#5a6a7a;font-weight:600;margin-bottom:.8rem}
.row{display:flex;justify-content:space-between;align-items:center;padding:.3rem 0;border-bottom:1px solid #1e1e30}
.row:last-child{border-bottom:none}
.lbl{color:#8a9aaa;font-size:.82rem}
.val{font-family:"SF Mono","Fira Mono",monospace;font-size:.82rem;color:#7ac0ff}
.proto-list{display:flex;flex-wrap:wrap;gap:.4rem}
.proto{background:#1a2a40;color:#7ac0ff;border:1px solid #2a3a50;border-radius:4px;padding:.2rem .5rem;font-size:.75rem;font-family:monospace}
table{width:100%;border-collapse:collapse}
th{font-size:.7rem;text-transform:uppercase;letter-spacing:.06em;color:#5a6a7a;padding:.4rem .6rem;border-bottom:1px solid #2a2a40;text-align:left}
td{padding:.4rem .6rem;border-bottom:1px solid #1e1e30;vertical-align:middle}
td:first-child{font-family:monospace;color:#7ac0ff;font-size:.82rem}
.method{font-family:monospace;color:#f0a050;font-size:.75rem}
.desc{color:#8a9aaa;font-size:.82rem}
tr:hover td{background:#1a1a28}
.footer{color:#3a4a5a;font-size:.75rem;margin-top:1.5rem}
.footer a{color:#4a7a9a;text-decoration:none}
.footer a:hover{color:#7ac0ff}
</style>
</head>
<body>

""";

    public const string Foot = "</body></html>\n";
}
