<?php
// PixPanel tarafından üretildi.
// Panelin verdiği tek kullanımlık anahtarı okuyup phpMyAdmin oturumunu başlatır.
// Anahtar dosyası yalnızca bir kez ve kısa süre içinde kullanılabilir.

const TOKEN_DIR = '{{token_dir}}';
const TOKEN_TTL = 60;

function fail(string $message): never
{
    http_response_code(403);
    header('Content-Type: text/html; charset=utf-8');
    echo '<!doctype html><meta charset="utf-8"><title>PixPanel</title>'
        . '<body style="font-family:system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 1rem;color:#15202b">'
        . '<h1 style="font-size:1.25rem">' . htmlspecialchars($message) . '</h1>'
        . '<p>Veritabanını panelin Veritabanları sayfasından yeniden açın.</p></body>';
    exit;
}

$token = $_GET['token'] ?? '';
if (!is_string($token) || !preg_match('/^[a-f0-9]{48}$/', $token)) {
    fail('Oturum sona erdi');
}
$path = TOKEN_DIR . '/' . $token . '.json';
$raw = @file_get_contents($path);
$fresh = $raw !== false && (time() - (int) @filemtime($path)) <= TOKEN_TTL;
@unlink($path);
$data = $fresh ? json_decode($raw, true) : null;
if (!is_array($data) || ($data['tool'] ?? '') !== 'phpmyadmin') {
    fail('Bağlantının süresi doldu');
}

session_set_cookie_params(['path' => '/tools/phpmyadmin/', 'secure' => {{secure}}, 'httponly' => true, 'samesite' => 'Strict']);
session_name('PixPanelPmaSignon');
session_start();
$_SESSION['PMA_single_signon_user'] = $data['user'];
$_SESSION['PMA_single_signon_password'] = $data['password'];
$_SESSION['PMA_single_signon_host'] = 'localhost';
session_write_close();

header('Location: /tools/phpmyadmin/index.php?db=' . rawurlencode($data['database']));
