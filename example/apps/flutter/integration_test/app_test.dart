// Runs on a device, emulator or desktop: `flutter test integration_test -d <device>`.
// Proves that the build hook bundled the core for that platform and that
// calls, streams and callbacks work in the app's UI isolate.
import 'package:demo_flutter_example/main.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('loads the bundled core and talks to it', (tester) async {
    await tester.pumpWidget(const DemoExampleApp());
    expect(find.textContaining('abi 1.'), findsOneWidget); // core description
    for (var i = 0; i < 100 && find.text('customer: Erika Mustermann').evaluate().isEmpty; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(find.text('customer: Erika Mustermann'), findsOneWidget);
    for (var i = 0; i < 100 && find.textContaining('created: Max Mustermann').evaluate().isEmpty; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(find.textContaining('created: Max Mustermann'), findsOneWidget);
    expect(find.textContaining('event: ClientEventCustomerUpdated'), findsOneWidget);
    expect(find.textContaining('signed with key'), findsOneWidget); // host calls reached the app
  });
}
