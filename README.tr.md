# proxyforge

**HTTP**, **HTTPS**, **SOCKS4** ve **SOCKS5** proxy'leri için hızlı ve eşzamanlı
proxy checker. Rust ile yazıldı.

[![CI](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml/badge.svg)](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml)
[![Lisans: MIT](https://img.shields.io/badge/lisans-MIT-blue.svg)](LICENSE)

English documentation: [README.md](README.md)

![proxyforge etkileşimli panosu](docs/tui.png)

## Özellikler

- HTTP, HTTPS (`CONNECT` üzerinden), SOCKS4/4a, SOCKS5 ve SOCKS5h desteği.
- Canlı ilerleme, filtreleme, sıralama, dışa aktarma ve mouse desteğiyle etkileşimli terminal panosu (`--tui`).
- URL'lerden doğrudan liste çekme (`--input URL`); düz metin, HTML ve JSON içinden ayıklar.
- Şemasız listeler için protokol otomatik tanıma (`--detect`).
- Offline GeoIP zenginleştirme (`--geo-db`) ve ülke/ASN filtreleri.
- Dönel çoklu judge ve yanıt doğrulaması (`--judge`, `--expect`).
- Canlı havuzu döndüren yerel proxy sunucusu (`--serve`).
- Sağ kalanları `--save` ile uç nokta, JSON veya CSV olarak kaydeder.
- Panoda (`S`) canlı ayar düzenleme; Enter toplu işi yeni değerlerle baştan başlatır.
- HTTP (`Proxy-Authorization`) ve SOCKS5 için kullanıcı adı/şifre doğrulaması.
- İşçi havuzu sayesinde yüksek eşzamanlılık — yavaş bir proxy sırayı tıkamaz.
- Proxy başına zaman aşımı, isteğe bağlı yeniden deneme ve doğru gecikme ölçümü.
- Anonimlik sınıflandırması: **transparent**, **anonymous**, **elite** veya bilinmiyor.
- Renkli tablo, JSON, JSONL ve CSV çıktıları.
- Yalnızca canlı olanlar, maksimum gecikme ve minimum anonimlik filtreleri.
- `Ctrl-C` ile temiz kesme: o ana kadarki sonuçlar yine de raporlanır.
- Global HTTP istemcisi veya bağlantı havuzu yok; her proxy kendi bağlantısında
  ölçülür.

## Kurulum

Her [sürümden](https://github.com/Shidori5417/proxyforge/releases) Linux, macOS
ve Windows için derlenmiş binary'ler indirilebilir.

Güncel bir kararlı Rust sürümü gerekir (1.85+).

```bash
git clone https://github.com/Shidori5417/proxyforge.git
cd proxyforge
cargo install --path .
```

Veya release derleyin:

```bash
cargo build --release
./target/release/proxyforge --help
```

Docker:

```bash
docker build -t proxyforge https://github.com/Shidori5417/proxyforge.git
docker run --rm -i proxyforge --input - --quiet < proxies.txt
```

Kabuk tamamlamaları:

```bash
proxyforge completions bash      # ayrıca zsh, fish, elvish, powershell
```

## Hızlı başlangıç

```bash
# İki uç noktayı kontrol et
proxyforge 1.2.3.4:8080 socks5://5.6.7.8:1080

# 200 paralel işçiyle bir listeyi kontrol et
proxyforge --input proxies.txt --concurrency 200

# Sadece hızlı ve elite proxy'leri tutup dışa aktar
proxyforge -i proxies.txt --alive-only --max-latency 800 \
  --min-anonymity elite --format json --output elite.json

# Listeyi borudan al, stdout'a sadece uç noktaları yaz
cat proxies.txt | proxyforge -i - --quiet > alive.txt

# Listeleri doğrudan web'den çek, Türkiye çıkışlarını tut ve kaydet
proxyforge -i https://example.com/proxies.txt --geo-db GeoLite2-Country.mmdb \
  --geo-db GeoLite2-ASN.mmdb --country tr --save tr.txt

# Şemasız bir listeyi kontrol et, sonra sağ kalanları yerel döndürücü olarak sun
proxyforge -i plain.txt --detect --serve 127.0.0.1:8080 --rotate-on-error
curl -x http://127.0.0.1:8080 https://httpbin.org/get
```

## Girdi biçimleri

Dosyada satır başına bir uç nokta veya argüman olarak birer uç nokta:

```text
1.2.3.4:8080                       # 8080 portunda HTTP proxy
http://1.2.3.4:3128
https://proxy.example.com:443
socks4://1.2.3.4:1080
socks5://1.2.3.4:1080
socks5h://1.2.3.4:1080             # DNS'i proxy çözer
socks5://user:pass@1.2.3.4:1080
http://user:p%40ss@1.2.3.4:8080    # yüzde kaçışları çözülür
[2001:db8::1]:8080                 # köşeli parantezli IPv6
```

Boş satırlar ve `#` ile başlayan satırlar yok sayılır. Tekrarlar ayıklanır,
çözülemeyen satırlar uyarı olarak bildirilir.

## Kullanım

```text
proxyforge [SEÇENEKLER] [PROXY]...
```

| Seçenek | Açıklama |
| --- | --- |
| `-i, --input <DOSYA/URL>` | Uç noktaları dosya, URL veya stdin'den oku; tekrarlanabilir |
| `-j, --judge <URL>` | Her proxy üzerinden istenen judge URL'si; tekrarlanabilir |
| `-c, --concurrency <N>` | Paralel kontrol sayısı (varsayılan `100`) |
| `-t, --timeout <SN>` | Proxy başına zaman aşımı (varsayılan `8`) |
| `-r, --retries <N>` | Başarısızlıktan sonra ek deneme sayısı (varsayılan `0`) |
| `-f, --format <BİÇİM>` | `table`, `json`, `jsonl` veya `csv` (varsayılan `table`) |
| `-o, --output <DOSYA>` | Raporu dosyaya yaz |
| `--alive-only` | Sadece yanıt veren proxy'leri raporla |
| `--max-latency <MS>` | Bundan yavaş proxy'leri ele |
| `--min-anonymity <SEVİYE>` | `transparent`, `anonymous` veya `elite` |
| `--expect <METİN>` | Judge yanıtı bu metni içermiyorsa proxy'yi ölü say |
| `--detect` | Şemasız uç noktalar için HTTP/SOCKS5/SOCKS4dene |
| `--protocols <LİSTE>` | Sadece bu protokolleri kontrol et (virgülle) |
| `--sort <SIRA>` | `latency` (varsayılan), `proxy` veya `status` |
| `--save <DOSYA>` | Çalıştıktan sonra sadece canlı proxy'leri dosyaya yaz |
| `--save-append` | Kayıt dosyasının üzerine yazmak yerine sona ekle |
| `--save-format <BÇM>` | `endpoints` (varsayılan), `json` veya `csv` |
| `--geo-db <DOSYA>` | MaxMind mmdb biçiminde offline GeoIP veritabanı; tekrarlanabilir |
| `--country <KODLAR>` | Sadece bu ülkelerden çıkan proxy'leri raporla |
| `--exclude-country <KODLAR>` | Bu ülkelerden çıkan proxy'leri ele |
| `--no-anonymity` | Anonimlik tespitini atla (bir istek tasarrufu) |
| `--local-ip-url <URL>` | Yerel genel IP'yi öğrenmek için bir kez kullanılan URL |
| `--show-credentials` | `***:***` yerine kimlik bilgilerini yazdır |
| `--tui` | Etkileşimli panoyu aç |
| `-q, --quiet` | Sadece canlı uç noktaları yazdır, tablo ve özet yok |
| `--no-summary` | Özet bloğunu gizle |
| `--no-progress` | İlerleme çubuğunu gizle |
| `--no-color` | ANSI renklerini kapat |
| `--serve <ADRES>` | Canlı havuzu yerel dönen proxy olarak sun |
| `--rotate <N>` | Her N istekte bir döndür (varsayılan `1`) |
| `--rotate-random` | Havuz üyelerini rastgele seç |
| `--rotate-on-error` | Hata durumunda sıradaki havuz üyesiyle yeniden dene |

## Çıktı

Tablo stdout'a, özet stderr'e gider; böylece makine tarafından okunan çıktı temiz
kalır:

```text
╭────────────────────────┬───────┬────────┬─────────┬─────────────┬────────────────┬──────────╮
│ Proxy                  ┆ Proto ┆ Status ┆ Latency ┆ Anonymity   ┆ Exit IP        ┆ Detail   │
╞════════════════════════╪═══════╪════════╪═════════╪═════════════╪════════════════╪══════════╡
│ socks5://1.2.3.4:1080  ┆ socks5┆ alive  ┆  142 ms ┆ elite       ┆ 203.0.113.9    ┆ HTTP 200 │
╰────────────────────────┴───────┴────────┴─────────┴─────────────┴────────────────┴──────────╯

checked 1 proxies in 142 ms
alive 1 (100.0%) · dead 0 (0.0%)
latency avg 142 ms · fastest 142 ms · slowest 142 ms
anonymity elite 1 · anonymous 0 · transparent 0 · unknown 0
```

![proxyforge; HTTP, SOCKS4 ve SOCKS5 proxy'lerinden oluşan bir listeyi kontrol ediyor](docs/demo.png)

JSON çıktısı bir nesne dizisidir:

```json
[
  {
    "proxy": "socks5://***:***@1.2.3.4:1080",
    "protocol": "socks5",
    "host": "1.2.3.4",
    "port": 1080,
    "alive": true,
    "latency_ms": 142,
    "status_code": 200,
    "anonymity": "elite",
    "exit_ip": "203.0.113.9",
    "error": null
  }
]
```

CSV aynı alanları kullanır. Kimlik bilgilerini çıktıda tutmak için
`--show-credentials` kullanın.

## Sağ kalanları kaydetme

`--save`, yalnızca yanıt veren proxy'leri — tespit edilen şemaları ve kimlik
bilgileri dahil — yazar; dosya doğrudan geri beslenebilir:

```bash
proxyforge -i proxies.txt --detect --save alive.txt
proxyforge -i alive.txt --alive-only --country tr --save-append tr.txt
```

`--save-format json` (veya `csv`) tüm alanları tutar. `Ctrl-C` ile kesmek de o
ana kadar kontrol edilenleri kaydeder; panoda `e` tuşu aynı dosyaya yazar.

## Etkileşimli pano

`--tui`, sayfanın başında görünen canlı panoyu açar:

| Tuş | İşlev |
| --- | --- |
| `q` / `Esc` | Çık (ilk `Esc` yalnızca yardım penceresini kapatır) |
| `↑` `↓` / `j` `k` | İmleci hareket ettir |
| `PgUp` / `PgDn` | Bir sayfa kaydır |
| `g` / `G` | İlk / son proxy'ye git |
| `f` | Filtreyi değiştir: all → alive → dead |
| `s` | Sıralamayı değiştir: latency → proxy → status |
| `e` | Görünen canlı uç noktaları `--save` dosyasına yaz |
| `r` | Seçili proxy'yi yeniden kontrol et |
| `R` | Tüm ölü uç noktaları yeniden kontrol et |
| `S` | Ayar panelini aç: judge, zaman aşımı, eşzamanlılık, deneme, tespit |
| tekerlek / tıklama | Sonuçları kaydır / satır seç |
| `?` | Yardım penceresini aç/kapat |

Ayar panelinde `↑↓` alan seçer, `◀▶` veya rakamlar değeri değiştirir, `Enter`
toplu işi yeni değerlerle baştan başlatır, `Esc` vazgeçer. Geçersiz giriş
footer'da bildirilir ve hiçbir şeyi değiştirmez.

Ayrıntı panosu imleci takip eder; tam hatayı, sızan yönlendirme başlıklarını ve
çıkış IP'sini gösterir. Çıkış kalan kontrolleri iptal eder ve terminali eski
hâline döndürür. `--format` ve `--output` gibi rapor seçenekleri yalnızca
etkileşimli olmayan modda geçerlidir.

## GeoIP zenginleştirme

`--geo-db`, MaxMind biçimindeki (`.mmdb`) veritabanlarını yükler ve her çıkış
IP'sine ülke, ISO kodu, ASN ve kuruluş bilgisini ekler. GeoLite2 Country ve ASN
kutudan çıkar; kayıt gerektirmeyen DB-IP Lite gibi ücretsiz alternatifler de
çalışır:

```bash
curl -O https://download.db-ip.com/free/dbip-country-lite-2026-11.mmdb.gz
curl -O https://download.db-ip.com/free/dbip-asn-lite-2026-11.mmdb.gz
gunzip dbip-*.mmdb.gz

proxyforge -i proxies.txt --geo-db dbip-country-lite-2026-11.mmdb \
  --geo-db dbip-asn-lite-2026-11.mmdb
```

Raporlara `Geo` sütunu (`DE · AS3320`), JSON/CSV çıktısına `country`,
`country_code`, `asn` ve `as_org` alanları eklenir. `--country TR,DE` ve
`--exclude-country CN` ISO koduna göre filtreler; veritabanı yüklenmediğinde
bu filtreler hiçbir proxy'yi geçirmeyecek şekilde davranır.

## Dönen proxy sunucusu

`--serve`, bir kontrolün sağ kalanlarını her isteği veya `CONNECT` tünelini
havuzdan ileten yerel bir HTTP proxy'ye dönüştürür:

```bash
proxyforge -i proxies.txt --alive-only --serve 127.0.0.1:8080 \
  --rotate 5 --rotate-on-error

curl -x http://127.0.0.1:8080 https://httpbin.org/get
```

Dönme, her `--rotate N` istekte sırayla ilerler; `--rotate-random` rastgele
seçer (her N istekte yeni bir rastgele üye). `--rotate-on-error` ile başarısız
üye atlanır ve sıradaki isteği yeniden dener. `Ctrl-C` sunucuyu durdurur.

`--rotate` için sayılan birim:

- **Düz HTTP istekleri** istek başına sayılır. Her biri `Connection: close` ile
  iletilir (istemcinin `Connection`, `Keep-Alive` ve `Proxy-Connection`
  başlıkları atılır), yanıt `Content-Length` / chunked çerçevelemesine uyularak
  geri akıtılır ve ardından istemci bağlantısı kapatılır. Keep-alive istemciler
  yeniden bağlanır; böylece her istek kendi havuz seçimini alır.
- **`CONNECT` tünelleri** opak bayt akışlarıdır ve tünel başına sayılır: bir
  tünelin içindeki tüm trafik (örneğin bir HTTPS oturumunun bütün istekleri)
  aynı havuz üyesini kullanır.

## Anonimlik seviyeleri

Checker kendi genel IP'nizi bir kez (proxy'siz) öğrenir ve her proxy üzerinden
judge'ın gördüğü bilgiyle karşılaştırır:

| Seviye | Anlamı |
| --- | --- |
| `elite` | Ne yönlendirme başlığı ne de istemci IP'si görünür |
| `anonymous` | Yönlendirme başlıkları (`X-Forwarded-For`, `Via`, …) var ama istemci IP'si gizli |
| `transparent` | Genel IP'niz köken olarak veya bir başlık içinde görünür |
| `unknown` | Judge yeterli bilgi yansıtmadı |

Tespit için istek başlıklarını yansıtan bir judge gerekir; varsayılan
`https://httpbin.org/get` bunu yapar. Sadece IP döndüren judge'lar
(`https://api.ipify.org` gibi) proxy'nin çalıştığını kanıtlar ama seviyeyi
`unknown` bırakır. Ek sorguyu `--no-anonymity` ile kapatabilirsiniz.

> Checker ile aynı ağda çalışan bir proxy, çıkış IP'si sizin IP'niz olacağı için
> haklı olarak `transparent` görünebilir.

## Nasıl çalışır

```
src/
├── proxy.rs      # uç nokta ayrıştırma ve maskeleme
├── transport.rs  # HTTP/1.1 istemcisi: CONNECT, SOCKS4a, SOCKS5, TLS
├── checker.rs    # işçi havuzu, yeniden deneme, gecikme, protokol tespiti
├── anonymity.rs  # judge yanıtı analizi
├── geo.rs        # offline GeoIP sorguları
├── serve.rs      # dönen yerel proxy sunucusu
├── output.rs     # filtreleme, sıralama, tablo/JSON/CSV
├── tui/          # pano durumu, çizim ve olay döngüsü
├── input.rs      # argüman, dosya ve stdin yükleme
└── main.rs       # CLI bağlantısı, ilerleme ve Ctrl-C
```

Taşıma katmanı bilinçli olarak küçüktür: tek `GET`, kontrol başına tek bağlantı,
havuzlama ve yönlendirme takibi yok. Böylece hatalar doğrudan proxy'ye atfedilebilir.

## Çıkış kodları

| Kod | Anlamı |
| --- | --- |
| `0` | Çalışma tamamlandı |
| `1` | Geçersiz argüman, okunamayan girdi veya geçersiz judge URL'si |
| `130` | `Ctrl-C` ile kesildi; kısmi sonuçlar yazıldı |

## Geliştirme

```bash
cargo test            # birim ve entegrasyon testleri, tamamen çevrimdışı
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Entegrasyon paketi, rastgele loopback portlarında yerel mock HTTP, SOCKS4 ve
SOCKS5 proxy'leri ile bir mock judge başlatır.

## Lisans

[MIT](LICENSE)
