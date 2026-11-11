-- PixPanel: bir iletinin hangi alan adı adına DKIM ile imzalanabileceğine karar verir.
--
-- Rspamd'ın hazır kuralı, sunucunun içinden gönderilen postayı gönderen başlığındaki alan adına
-- bakarak imzalar. Paylaşımlı sunucuda bu, bir müşterinin PHP'sinin başka müşterinin alan adı
-- adına geçerli imzalı posta göndermesi demektir. Burada imza yalnızca gönderenin o alan adı
-- üzerinde hakkı olduğu kanıtlanınca atılır:
--
--   * Parolayla oturum açan gönderen (SMTP): yalnızca kendi adresinin alan adı.
--   * Sunucunun içinden `sendmail` ile gönderen: Postfix, yalnızca bu yoldan gelen iletinin en
--     başına gönderenin Linux kullanıcı numarasını taşıyan bir başlık ekler (X-PixPanel-Uid); o
--     kullanıcının hesabına ait alan adları panelin yazdığı eşleme dosyasından okunur. Gönderenin
--     kendi yazdığı aynı adlı başlık ya silinir (SMTP yolu) ya da gerçeğinin altında kalır; yalnızca
--     en üstteki okunur.
--   * Bunların dışındaki her şey (dışarıdan gelen posta, kimliği belirsiz yerel bağlantı) imzalanmaz.
-- Rspamd bu metni her ileti için doğrudan çağırır; ileti ilk argümandır.
local task = ...
do
  -- İç başlık alıcıya gitmez.
  local marks = task:get_header_full('X-PixPanel-Uid')
  if marks and marks[1] then
    task:set_milter_reply({ remove_headers = { ['X-PixPanel-Uid'] = 0 } })
  end

  local from = task:get_from('mime')
  if not from or not from[1] or not from[1].domain then
    return false
  end
  local domain = string.lower(from[1].domain)
  if not string.match(domain, '^[%w%.%-]+$') then
    return false
  end

  local allowed = false
  local user = task:get_user()
  if user then
    local user_domain = string.match(user, '@([^@]+)$')
    allowed = user_domain ~= nil and string.lower(user_domain) == domain
  else
    local ip = task:get_from_ip()
    if ip and ip:is_valid() and not ip:is_local() then
      return false
    end
    local first = marks and marks[1] and (marks[1].decoded or marks[1].value) or ''
    local uid = string.match(first, '^%s*(%d+)%s*$')
    if uid == '0' then
      -- root (zamanlanmış sistem görevleri, panelin kendisi) güvenilirdir.
      allowed = true
    elseif uid then
      -- Eşleme dosyası kısa aralıklarla yeniden okunur; panel hesap ya da alan adı değişince yeniden yazar.
      local now = os.time()
      if not pix_dkim_owners or now - (pix_dkim_loaded or 0) > 20 then
        local table_ = {}
        local file = io.open('{{map}}', 'r')
        if file then
          for line in file:lines() do
            table_[line] = true
          end
          file:close()
        end
        pix_dkim_owners = table_
        pix_dkim_loaded = now
      end
      allowed = pix_dkim_owners[uid .. ':' .. domain] == true
    end
  end
  if not allowed then
    return false
  end

  local path = '{{dkim_dir}}/' .. domain .. '.{{selector}}.key'
  local key = io.open(path, 'r')
  if not key then
    return false
  end
  key:close()
  return { domain = domain, selector = '{{selector}}', key = path }
end
