<?php
// PixPanel tarafından üretildi.
// Adminer'i panelin verdiği tek kullanımlık anahtarla, oturum açma formu olmadan başlatır.

const TOKEN_DIR = '{{token_dir}}';
const TOKEN_TTL = 60;

function pix_fail(string $message): never
{
    http_response_code(403);
    header('Content-Type: text/html; charset=utf-8');
    echo '<!doctype html><meta charset="utf-8"><title>PixPanel</title>'
        . '<body style="font-family:system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 1rem;color:#15202b">'
        . '<h1 style="font-size:1.25rem">' . htmlspecialchars($message) . '</h1>'
        . '<p>Veritabanını panelin Veritabanları sayfasından yeniden açın.</p></body>';
    exit;
}

// Kimlik bilgileri Adminer'in kendi oturumundan ayrı bir oturumda saklanır.
session_set_cookie_params(['path' => '/tools/adminer/', 'secure' => {{secure}}, 'httponly' => true, 'samesite' => 'Strict']);
session_name('PixPanelAdminer');
session_start();

$token = $_GET['token'] ?? null;
if (is_string($token)) {
    if (!preg_match('/^[a-f0-9]{48}$/', $token)) {
        pix_fail('Oturum sona erdi');
    }
    $path = TOKEN_DIR . '/' . $token . '.json';
    $raw = @file_get_contents($path);
    $fresh = $raw !== false && (time() - (int) @filemtime($path)) <= TOKEN_TTL;
    @unlink($path);
    $data = $fresh ? json_decode($raw, true) : null;
    if (!is_array($data) || ($data['tool'] ?? '') !== 'adminer') {
        pix_fail('Bağlantının süresi doldu');
    }
    $_SESSION['pix'] = ['user' => $data['user'], 'password' => $data['password'], 'database' => $data['database'], 'fresh' => true];
    session_write_close();
    header('Location: /tools/adminer/?pgsql=127.0.0.1&username=' . rawurlencode($data['user']) . '&db=' . rawurlencode($data['database']) . '&ns=public');
    exit;
}

$pix = $_SESSION['pix'] ?? null;
if (!is_array($pix)) {
    session_write_close();
    pix_fail('Oturum sona erdi');
}
// Oturum burada kapatılmaz: Adminer, önceden başlatılmış bir oturum görürse
// kendi oturumunu açmaz ve parolayı bu oturumda saklar.
$first = !empty($pix['fresh']);
$_SESSION['pix']['fresh'] = false;
$GLOBALS['pix_credentials'] = $pix;
// Bağlantı açıldıktan sonraki ilk istekte giriş formu gönderilmiş gibi davranılır.
if ($first && !isset($_POST['auth'])) {
    $_POST['auth'] = ['driver' => 'pgsql', 'server' => '127.0.0.1', 'username' => $pix['user'], 'password' => $pix['password'], 'db' => $pix['database'], 'permanent' => ''];
}

function adminer_object()
{
    class PixPanelAdminer extends Adminer\Adminer
    {
        public function name()
        {
            return 'PixPanel · Adminer';
        }

        public function credentials()
        {
            $pix = $GLOBALS['pix_credentials'];
            return ['127.0.0.1', $pix['user'], $pix['password']];
        }

        public function login($login, $password)
        {
            return hash_equals($GLOBALS['pix_credentials']['user'], (string) $login);
        }

        public function databases($flush = true)
        {
            return [$GLOBALS['pix_credentials']['database']];
        }

        public function loginForm()
        {
            echo '<p>Oturum sona erdi. Veritabanını panelin Veritabanları sayfasından yeniden açın.</p>';
        }
    }

    return new PixPanelAdminer();
}

include '{{adminer_script}}';
