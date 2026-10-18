//! `kobi-ctl` : la CLI de Kobi en binaire console. Sous Windows, l'app
//! `kobi.exe` est un exécutable graphique : dans un terminal, `kobi-ctl`
//! rend la main proprement et donne son code de retour.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kobi_ctl::cli::main_avec(&args));
}
