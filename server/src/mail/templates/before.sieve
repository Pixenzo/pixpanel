# PixPanel tarafından üretildi.
require ["fileinto", "mailbox"];
if header :is "X-Spam" "Yes" {
    fileinto :create "Junk";
    stop;
}
