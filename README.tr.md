# proxyforge

**HTTP**, **HTTPS**, **SOCKS4** ve **SOCKS5** proxy'leri için hızlı ve eşzamanlı
proxy checker. Rust ile yazıldı.

[![CI](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml/badge.svg)](https://github.com/Shidori5417/proxyforge/actions/workflows/ci.yml)
[![Lisans: MIT](https://img.shields.io/badge/lisans-MIT-blue.svg)](LICENSE)

English documentation: [README.md](README.md)

![proxyforge; HTTP, SOCKS4 ve SOCKS5 proxy'lerinden oluşan bir listeyi kontrol ediyor](docs/demo.png)

## Özellikler

- HTTP, HTTPS (`CONNECT` üzerinden), SOCKS4/4a, SOCKS5 ve SOCKS5h desteği.
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
| `-i, --input <DOSYA>` | Uç noktaları dosyadan oku; `-` stdin demektir |
| `-j, --judge <URL>` | Her proxy üzerinden istenen URL (varsayılan `https://httpbin.org/get`) |
| `-c, --concurrency <N>` | Paralel kontrol sayısı (varsayılan `100`) |
| `-t, --timeout <SN>` | Proxy başına zaman aşımı (varsayılan `8`) |
| `-r, --retries <N>` | Başarısızlıktan sonra ek deneme sayısı (varsayılan `0`) |
| `-f, --format <BİÇİM>` | `table`, `json`, `jsonl` veya `csv` (varsayılan `table`) |
| `-o, --output <DOSYA>` | Raporu dosyaya yaz |
| `--alive-only` | Sadece yanıt veren proxy'leri raporla |
| `--max-latency <MS>` | Bundan yavaş proxy'leri ele |
| `--min-anonymity <SEVİYE>` | `transparent`, `anonymous` veya `elite` |
| `--protocols <LİSTE>` | Sadece bu protokolleri kontrol et (virgülle) |
| `--sort <SIRA>` | `latency` (varsayılan), `proxy` veya `status` |
| `--no-anonymity` | Anonimlik tespitini atla (bir istek tasarrufu) |
| `--local-ip-url <URL>` | Yerel genel IP'yi öğrenmek için bir kez kullanılan URL |
| `--show-credentials` | `***:***` yerine kimlik bilgilerini yazdır |
| `-q, --quiet` | Sadece canlı uç noktaları yazdır, tablo ve özet yok |
| `--no-summary` | Özet bloğunu gizle |
| `--no-progress` | İlerleme çubuğunu gizle |
| `--no-color` | ANSI renklerini kapat |

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
├── checker.rs    # işçi havuzu, yeniden deneme, gecikme
├── anonymity.rs  # judge yanıtı analizi
├── output.rs     # filtreleme, sıralama, tablo/JSON/CSV
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
