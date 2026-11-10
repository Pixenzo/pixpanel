<?php
// PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.
// phpMyAdmin'e yalnızca panelden, tek kullanımlık bağlantıyla girilir.
$cfg['Servers'][1]['auth_type'] = 'signon';
$cfg['Servers'][1]['SignonSession'] = 'PixPanelPmaSignon';
$cfg['Servers'][1]['SignonCookieParams'] = ['path' => '/tools/phpmyadmin/', 'secure' => {{secure}}, 'httponly' => true, 'samesite' => 'Strict'];
$cfg['Servers'][1]['SignonURL'] = '/tools/phpmyadmin/pixpanel-signon.php';
$cfg['Servers'][1]['LogoutURL'] = '/tools/phpmyadmin/pixpanel-signon.php';
$cfg['Servers'][1]['host'] = 'localhost';
$cfg['Servers'][1]['connect_type'] = 'socket';
$cfg['Servers'][1]['AllowNoPassword'] = false;
// Yapılandırma deposu kullanılmaz; geçici hesaplar ona erişemez.
$cfg['Servers'][1]['controluser'] = '';
$cfg['Servers'][1]['controlpass'] = '';
$cfg['Servers'][1]['pmadb'] = '';
$cfg['PmaNoRelation_DisableWarning'] = true;
$cfg['VersionCheck'] = false;
$cfg['ShowServerInfo'] = false;
$cfg['TempDir'] = '{{temp_dir}}';
$cfg['UploadDir'] = '';
$cfg['SaveDir'] = '';
