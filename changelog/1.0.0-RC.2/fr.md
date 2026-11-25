Deuxième version candidate de Vixeeny 1.0. Signalez tout ce qui ne va pas : Paramètres → À propos → Signaler un problème.

## Corrections

- Les captures de jeux en plein écran sans bordure n'ont plus de bandes noires intermittentes en haut de l'image.
- Les jeux en plein écran risquent moins de se bloquer ou de figer au lancement pendant que le replay démarre : les images capturées et les ressources du GPU sont libérées tout de suite, la capture des fenêtres secondaires reste désactivée pour les jeux, et la conversion par le GPU est sautée quand la file de l'encodeur est pleine.
- La détection des encodeurs matériels se fait un processus à la fois et n'attend plus que 15 secondes au lieu de 60.
- Les jeux protégés par un anti-triche ne sont plus pris pour fermés : le replay et sa notification de démarrage ne redémarrent plus en boucle.
- Passer d'un jeu à l'autre ou fermer un jeu pendant le replay passe proprement au jeu suivant au premier plan.
