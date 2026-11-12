<?php
// PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.
//
// Roundcube `password` eklentisi için sürücü: parolayı panelin oturumsuz ucuna
// (POST /api/v1/mail/public/password) gönderir. Panel, geçerli parolayı kendisi doğrular, parola
// kurallarını uygular ve deneme sınırını yönetir; bu sürücü yalnızca iletir.

class rcube_pixpanel_password
{
    public function save($currpass, $newpass, $username)
    {
        $rcmail = rcmail::get_instance();
        $url = $rcmail->config->get('pixpanel_api_url');
        $key = $rcmail->config->get('pixpanel_api_key');
        if (!$url) {
            return PASSWORD_CONNECT_ERROR;
        }

        $payload = json_encode([
            'address' => (string) $username,
            'current_password' => (string) $currpass,
            'new_password' => (string) $newpass,
        ]);
        $headers = [
            'Content-Type: application/json',
            'Accept: application/json',
            'X-PixPanel-Webmail-Key: ' . $key,
            // Ziyaretçinin gerçek adresi: panel deneme sınırını bu adrese göre uygular.
            'X-PixPanel-Client-Ip: ' . (isset($_SERVER['REMOTE_ADDR']) ? $_SERVER['REMOTE_ADDR'] : ''),
        ];
        $context = stream_context_create([
            'http' => [
                'method' => 'POST',
                'header' => implode("\r\n", $headers),
                'content' => $payload,
                'timeout' => 30,
                // 4xx yanıtlarının gövdesi de (hata iletisi) okunur.
                'ignore_errors' => true,
            ],
            // Panel aynı makinededir; sertifikası genellikle kendinden imzalıdır.
            'ssl' => ['verify_peer' => false, 'verify_peer_name' => false, 'allow_self_signed' => true],
        ]);

        $body = @file_get_contents($url, false, $context);
        if ($body === false || empty($http_response_header)) {
            rcube::raise_error('PixPanel parola sürücüsü panele ulaşamadı', true, false);
            return PASSWORD_CONNECT_ERROR;
        }

        $status = 0;
        if (preg_match('#^HTTP/\S+\s+(\d{3})#', $http_response_header[0], $matches)) {
            $status = (int) $matches[1];
        }
        if ($status >= 200 && $status < 300) {
            return PASSWORD_SUCCESS;
        }

        $data = json_decode($body, true);
        $message = (is_array($data) && isset($data['error']['message'])) ? (string) $data['error']['message'] : '';
        if ($status === 429) {
            $message = $message ?: 'Çok fazla deneme yapıldı, lütfen biraz bekleyin';
        }
        if ($status >= 500 || $status === 0) {
            return PASSWORD_CONNECT_ERROR;
        }
        return $message !== '' ? ['code' => PASSWORD_ERROR, 'message' => $message] : PASSWORD_ERROR;
    }
}
