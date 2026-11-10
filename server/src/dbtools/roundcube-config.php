<?php
// PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.
$config = [];

// Web postasının kendi verileri (adres defteri, tercihler) ayrı bir SQLite dosyasında durur.
$config['db_dsnw'] = 'sqlite:///{{db_file}}?mode=0640';

// Posta sunucusu aynı makinededir; bağlantı yerel olduğundan sertifika adı doğrulanmaz.
$local_tls = ['ssl' => ['verify_peer' => false, 'verify_peer_name' => false, 'allow_self_signed' => true]];
$config['imap_host'] = 'ssl://127.0.0.1:993';
$config['imap_conn_options'] = $local_tls;
$config['smtp_host'] = 'tls://127.0.0.1:587';
$config['smtp_conn_options'] = $local_tls;
$config['smtp_user'] = '%u';
$config['smtp_pass'] = '%p';

$config['des_key'] = '{{des_key}}';
$config['product_name'] = 'PixPanel Web Postası';
$config['support_url'] = '';
$config['skin'] = 'elastic';
$config['language'] = 'tr_TR';
$config['plugins'] = ['archive', 'zipdownload'];
$config['enable_spellcheck'] = false;
$config['enable_installer'] = false;

// Panel üzerinden, alt yol altında yayınlanır.
$config['use_https'] = {{secure}};
$config['session_path'] = '/webmail/';
$config['session_samesite'] = 'Lax';
$config['temp_dir'] = '{{temp_dir}}';
$config['log_driver'] = 'syslog';

// Kutuların özel klasörleri Dovecot'taki adlarla eşleşir.
$config['drafts_mbox'] = 'Drafts';
$config['junk_mbox'] = 'Junk';
$config['sent_mbox'] = 'Sent';
$config['trash_mbox'] = 'Trash';
$config['archive_mbox'] = 'Archive';
$config['create_default_folders'] = true;
