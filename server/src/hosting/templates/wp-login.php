<?php
/**
 * PixPanel: panelden tek kullanımlık yönetici girişi.
 *
 * Panel, "Yönetim paneline gir" düğmesine basıldığında rastgele bir anahtar üretir,
 * özetini veritabanına yazar ve anahtarı içeren adresi yalnızca düğmeye basan kişiye
 * verir. Anahtar 60 saniye geçerlidir ve ilk kullanımda silinir.
 *
 * Bu dosyayı PixPanel yazar; elle değiştirirseniz bir sonraki girişte üzerine yazılır.
 */

if (!defined('ABSPATH')) {
    exit;
}

add_action('init', function () {
    if (empty($_GET['pixpanel_login']) || !is_string($_GET['pixpanel_login'])) {
        return;
    }
    $record = get_option('pixpanel_login_token');
    // Kayıt, anahtar doğru olsa da olmasa da ilk denemede silinir: deneme yanılma yapılamaz.
    delete_option('pixpanel_login_token');

    $valid = is_array($record)
        && !empty($record['hash']) && is_string($record['hash'])
        && !empty($record['expires']) && time() <= (int) $record['expires']
        && hash_equals($record['hash'], hash('sha256', $_GET['pixpanel_login']));
    if (!$valid) {
        wp_die('Giriş bağlantısının süresi dolmuş ya da bağlantı daha önce kullanılmış. Panelden yeniden deneyin.', 'PixPanel', array('response' => 403));
    }

    $user = get_user_by('id', (int) $record['user']);
    if (!$user || !user_can($user, 'manage_options')) {
        wp_die('Yönetici hesabı bulunamadı.', 'PixPanel', array('response' => 403));
    }

    wp_set_current_user($user->ID);
    wp_set_auth_cookie($user->ID, false);
    wp_safe_redirect(admin_url());
    exit;
});
