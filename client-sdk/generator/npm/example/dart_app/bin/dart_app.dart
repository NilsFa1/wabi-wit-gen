import 'package:demo_greeter/demo_greeter.dart';

Future<void> main() async {
  final greeter = DemoGreeter('Hallo');
  final g = await greeter.greet('Nils');
  print('${g.text} (${g.count})');
  try {
    await greeter.greet('');
  } on GreetError catch (e) {
    print('Fehler: $e');
  }
  greeter.close();
}
